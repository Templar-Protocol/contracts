//! `create_proposal_borsh` against a real node: parity, gas, and payload size.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_api::NetworkConfig;
use near_sdk::{json_types::Base64VecU8, json_types::U128, Gas, NearToken};
use rstest::rstest;
use serde_json::json;
use templar_common::{upgrade::UpgradeSource, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_near_governance_common::{
    proposal_body_size, FunctionCall, GovernancePolicy, MethodPolicy, Operation, Proposal,
    ReflexiveOperation, Role, ADMIN_RESERVED_PROPOSAL_BYTES, MAX_GOVERNANCE_WORKING_SET_BYTES,
};

use common::{
    call, call_borsh, deploy_with_init, self_upgrade, view, CreateProposalArgs, ProposalIdArgs,
    ONE_YOCTO,
};

/// The RPC's body cap answers 413 before the node applies `max_transaction_size`, so the assertion
/// below requires only an error.
const OVERSIZED_CODE_LEN: usize = 1_250_000;

/// `proxy_oracle_id` is never called: these tests only create proposals.
async fn governance(harness: &SandboxHarness) -> Result<(NetworkConfig, AccountId, AccountId)> {
    let gov = harness.create_user("gov").await?.0;
    let admin = harness.create_user("admin").await?.0;
    let policy = GovernancePolicy::uniform(Nanoseconds::zero())?;

    deploy_with_init(
        &harness.network,
        &gov,
        wasm::proxy_governance().await.to_vec(),
        "new",
        json!({ "proxy_oracle_id": "oracle.near", "admin_id": admin, "policy": policy }),
    )
    .await?;
    Ok((harness.network.clone(), gov, admin))
}

async fn stored_operation(
    network: &NetworkConfig,
    gov: &AccountId,
    id: u32,
) -> Result<Option<Operation>> {
    Ok(
        view::<Option<Proposal<Operation>>>(network, gov, "get_proposal", ProposalIdArgs { id })
            .await?
            .map(|proposal| proposal.operation),
    )
}

#[rstest]
#[tokio::test]
async fn borsh_stores_the_same_operation_for_less_gas(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (network, gov, admin) = governance(&harness).await?;
    let code = wasm::proxy_governance().await.to_vec();
    let code_len = code.len();
    let operation = self_upgrade(UpgradeSource::Code(Base64VecU8(code)));

    let json_gas = call(
        &network,
        &gov,
        "create_proposal",
        CreateProposalArgs {
            id: 0,
            operation: operation.clone(),
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?
    .total_gas_burnt;

    let borsh_gas = call_borsh(
        &network,
        &gov,
        "create_proposal_borsh",
        CreateProposalArgs {
            id: 1,
            operation: operation.clone(),
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?
    .total_gas_burnt;

    // Against `operation`, not each other: two missing proposals compare equal too.
    assert_eq!(
        stored_operation(&network, &gov, 0).await?.as_ref(),
        Some(&operation),
        "the json entrypoint should have stored the operation",
    );
    assert_eq!(
        stored_operation(&network, &gov, 1).await?.as_ref(),
        Some(&operation),
        "the borsh entrypoint should have stored the same operation",
    );

    println!("create_proposal over {code_len} bytes of wasm: json {json_gas}, borsh {borsh_gas}");
    assert!(
        borsh_gas < json_gas,
        "borsh should burn less gas than json: {borsh_gas} vs {json_gas}",
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn borsh_carries_a_payload_json_cannot(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let (network, gov, admin) = governance(&harness).await?;
    let operation = self_upgrade(UpgradeSource::Code(Base64VecU8(vec![
        0u8;
        OVERSIZED_CODE_LEN
    ])));

    let error = call(
        &network,
        &gov,
        "create_proposal",
        CreateProposalArgs {
            id: 0,
            operation: operation.clone(),
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await
    .expect_err("a json transaction this large should be rejected")
    .to_string();

    // Rejected for its size, before execution — an auth or decode regression would panic instead,
    // and would otherwise satisfy this test while proving nothing about the limit it is named for.
    let by_size = error.contains("413") || error.to_lowercase().contains("size");
    assert!(by_size, "expected a size rejection, got: {error}");

    // The rejected transaction never reached the contract, so id 0 is still next.
    call_borsh(
        &network,
        &gov,
        "create_proposal_borsh",
        CreateProposalArgs {
            id: 0,
            operation,
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    assert_eq!(
        view::<u32>(&network, &gov, "proposal_count", json!({})).await?,
        1,
    );

    // Release the ~12.5 NEAR of storage the blob staked.
    call(
        &network,
        &gov,
        "cancel_proposal",
        ProposalIdArgs { id: 0 },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    Ok(())
}

#[rstest]
#[tokio::test]
async fn maximum_byte_queue_allows_admin_revocation(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (gov, _) = harness
        .create_account_via_tx("byte-gov", NearToken::from_near(900))
        .await?;
    let admin = harness.create_user("byte-admin").await?.0;
    let delegate = harness.create_user("byte-delegate").await?.0;
    let network = &harness.network;
    deploy_with_init(
        network,
        &gov,
        wasm::proxy_governance().await.to_vec(),
        "new",
        json!({
            "proxy_oracle_id": "oracle.near",
            "admin_id": admin,
            "policy": GovernancePolicy::uniform(Nanoseconds::zero())?,
        }),
    )
    .await?;
    let policy = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_set_proxy".into(),
        policy: Some(MethodPolicy {
            ttl: Nanoseconds::zero(),
            role: Role::ProxyConfigurationManager,
        }),
    });
    let grant = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: delegate.clone(),
        role: Role::ProxyConfigurationManager,
        set: true,
    });
    for (id, operation) in [(0, policy), (1, grant)] {
        call(
            network,
            &gov,
            "create_proposal",
            CreateProposalArgs {
                id,
                operation,
                requested_ttl: Nanoseconds::zero(),
            },
            &admin,
            ONE_YOCTO,
        )
        .await?;
        call(
            network,
            &gov,
            "execute_proposal",
            ProposalIdArgs { id },
            &admin,
            ONE_YOCTO,
        )
        .await?;
    }
    let entries = harness
        .client()?
        .read(templar_gateway_methods_spec::account::ViewState {
            account_id: gov.clone(),
            prefix: b"STATE".to_vec().into(),
        })
        .await?;
    let state_size = u64::try_from(
        entries
            .iter()
            .find(|entry| entry.key.0 == b"STATE")
            .unwrap()
            .value
            .0
            .len(),
    )?;

    let target = |len| {
        Operation::TargetFunctionCall(FunctionCall {
            method_name: "admin_set_proxy".into(),
            args: Base64VecU8(vec![0; len]),
            attached_deposit: U128(0),
            gas: Gas::from_tgas(1),
        })
    };
    let revoke = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: delegate.clone(),
        role: Role::ProxyConfigurationManager,
        set: false,
    });
    let budget =
        MAX_GOVERNANCE_WORKING_SET_BYTES - ADMIN_RESERVED_PROPOSAL_BYTES - state_size - 62 * 8;
    let base = proposal_body_size(&target(0), &delegate)?;
    let mut remaining = budget - base * 62;
    for index in 0..62_u32 {
        let bytes = if index == 0 {
            1_250_000
        } else {
            remaining / u64::from(62 - index)
        };
        remaining -= bytes;
        call_borsh(
            network,
            &gov,
            "create_proposal_borsh",
            CreateProposalArgs {
                id: index + 2,
                operation: target(usize::try_from(bytes)?),
                requested_ttl: Nanoseconds::zero(),
            },
            &delegate,
            ONE_YOCTO,
        )
        .await?;
    }
    assert_eq!(remaining, 0);
    let admin_size = ADMIN_RESERVED_PROPOSAL_BYTES - proposal_body_size(&revoke, &admin)? - 16;
    let admin_args = admin_size - proposal_body_size(&target(0), &admin)?;
    call_borsh(
        network,
        &gov,
        "create_proposal_borsh",
        CreateProposalArgs {
            id: 64,
            operation: target(usize::try_from(admin_args)?),
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    call(
        network,
        &gov,
        "create_proposal",
        CreateProposalArgs {
            id: 65,
            operation: revoke,
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    call(
        network,
        &gov,
        "execute_proposal",
        ProposalIdArgs { id: 65 },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    let roles: Vec<Role> = view(
        network,
        &gov,
        "get_roles",
        json!({ "account_id": delegate }),
    )
    .await?;
    assert!(!roles.contains(&Role::ProxyConfigurationManager));
    assert_eq!(
        view::<u32>(network, &gov, "proposal_count", json!({})).await?,
        1
    );
    assert!(view::<Option<Proposal<Operation>>>(
        network,
        &gov,
        "get_proposal",
        ProposalIdArgs { id: 64 }
    )
    .await?
    .is_some());
    Ok(())
}

#[rstest]
#[tokio::test]
async fn unrecoverable_policy_edit_rolls_back_and_retains_proposal(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (network, gov, admin) = governance(&harness).await?;
    let method = "m".repeat(1_100_000);
    let operation = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: method.clone(),
        policy: Some(MethodPolicy {
            ttl: Nanoseconds::zero(),
            role: Role::Admin,
        }),
    });
    call_borsh(
        &network,
        &gov,
        "create_proposal_borsh",
        CreateProposalArgs {
            id: 0,
            operation,
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    let failure = near_api::Contract(gov.clone())
        .call_function("execute_proposal", json!({ "id": 0 }))
        .transaction()
        .deposit(ONE_YOCTO)
        .gas(Gas::from_tgas(300))
        .with_signer(admin, common::signer())
        .wait_until(templar_gateway_testing::TEST_FINALITY_POLICY.transaction_status())
        .send_to(&network)
        .await?
        .assert_failure();
    assert!(format!("{failure:?}").contains("Governance policy exceeds recoverable working set"));
    assert_eq!(
        view::<u32>(&network, &gov, "proposal_count", json!({})).await?,
        1
    );
    let policy: GovernancePolicy = view(&network, &gov, "get_governance_policy", json!({})).await?;
    assert!(!policy.method_policies().contains_key(&method));
    Ok(())
}

#[rstest]
#[tokio::test]
async fn cancellation_log_budget_prevents_long_method_revocation_failure(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    use templar_proxy_oracle_near_governance_common::{
        cancellation_event_size, ADMIN_RESERVED_CANCELLATION_LOG_BYTES,
        MAX_PENDING_CANCELLATION_LOG_BYTES,
    };

    let (network, gov, admin) = governance(&harness).await?;
    let delegate = harness.create_user("log-delegate").await?.0;
    let method = "m".repeat(256);
    let policy = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: method.clone(),
        policy: Some(MethodPolicy {
            ttl: Nanoseconds::zero(),
            role: Role::ProxyConfigurationManager,
        }),
    });
    let grant = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: delegate.clone(),
        role: Role::ProxyConfigurationManager,
        set: true,
    });
    for (id, operation) in [(0, policy), (1, grant)] {
        call(
            &network,
            &gov,
            "create_proposal",
            CreateProposalArgs {
                id,
                operation,
                requested_ttl: Nanoseconds::zero(),
            },
            &admin,
            ONE_YOCTO,
        )
        .await?;
        call(
            &network,
            &gov,
            "execute_proposal",
            ProposalIdArgs { id },
            &admin,
            ONE_YOCTO,
        )
        .await?;
    }
    let operation = Operation::TargetFunctionCall(FunctionCall {
        method_name: method,
        args: Base64VecU8(Vec::new()),
        attached_deposit: U128(0),
        gas: Gas::from_tgas(1),
    });
    let mut total = 0;
    let mut id = 2;
    while total + cancellation_event_size(id, &operation)?
        <= MAX_PENDING_CANCELLATION_LOG_BYTES - ADMIN_RESERVED_CANCELLATION_LOG_BYTES
    {
        total += cancellation_event_size(id, &operation)?;
        call_borsh(
            &network,
            &gov,
            "create_proposal_borsh",
            CreateProposalArgs {
                id,
                operation: operation.clone(),
                requested_ttl: Nanoseconds::zero(),
            },
            &delegate,
            ONE_YOCTO,
        )
        .await?;
        id += 1;
    }
    assert!(
        id < 65,
        "log bound must prevent the previously failing 63-body queue"
    );
    let args = near_sdk::borsh::to_vec(&CreateProposalArgs {
        id,
        operation,
        requested_ttl: Nanoseconds::zero(),
    })?;
    let failure = near_api::Contract(gov.clone())
        .call_function_raw("create_proposal_borsh", args)
        .transaction()
        .deposit(ONE_YOCTO)
        .gas(Gas::from_tgas(300))
        .with_signer(delegate.clone(), common::signer())
        .wait_until(templar_gateway_testing::TEST_FINALITY_POLICY.transaction_status())
        .send_to(&network)
        .await?
        .assert_failure();
    assert!(format!("{failure:?}").contains("Pending cancellation log byte limit exceeded"));
    let revoke = Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: delegate.clone(),
        role: Role::ProxyConfigurationManager,
        set: false,
    });
    call(
        &network,
        &gov,
        "create_proposal",
        CreateProposalArgs {
            id,
            operation: revoke,
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    call(
        &network,
        &gov,
        "execute_proposal",
        ProposalIdArgs { id },
        &admin,
        ONE_YOCTO,
    )
    .await?;
    let roles: Vec<Role> = view(
        &network,
        &gov,
        "get_roles",
        json!({ "account_id": delegate }),
    )
    .await?;
    assert!(!roles.contains(&Role::ProxyConfigurationManager));
    assert_eq!(
        view::<u32>(&network, &gov, "proposal_count", json!({})).await?,
        0
    );
    Ok(())
}
