//! Every operation the governance contract can execute, driven for real against a deployed proxy
//! oracle.
//!
//! `execute_proposal` dispatches on a detached promise, so a wrong method name or arg field fails on
//! its own receipt while the governance transaction still succeeds — hence assertions on the
//! oracle's observable state rather than on transaction status.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::json_types::Base64VecU8;
use rstest::rstest;
use serde_json::json;
use templar_common::{upgrade::UpgradeSource, Decimal, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreaker, CircuitBreakerSetConfig, CircuitBreakerStatus, StepwiseChange,
};
use templar_proxy_oracle_near_governance_common::{target, Operation, ReflexiveOperation, Role};

use common::{code_hash, deploy_global_contract, governed, self_upgrade, view, PRICE_ID};

/// `add` requires `breaker_id == next_id`, which starts at zero and only ever increments.
const BREAKER: u32 = 0;

fn breaker() -> CircuitBreaker {
    CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::ONE_HALF,
    })
}

/// Each distinct from the default it overwrites, so a silent no-op cannot read as a success.
const REARM_DELAY: Nanoseconds = Nanoseconds::from_secs(4_242);
const SAMPLE_INTERVAL: Nanoseconds = Nanoseconds::from_secs(60);

/// Every proxy/circuit-breaker builder in `target.rs`, each proved by the state change it makes on
/// the oracle. `admin_upgrade` is the one target method not here — it has no view to read back, so
/// it gets its own test below.
#[rstest]
#[tokio::test]
async fn every_target_builder_drives_its_admin_method(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_configure_circuit_breakers(
                PRICE_ID,
                CircuitBreakerSetConfig {
                    sample_interval_ns: SAMPLE_INTERVAL,
                    history_len: 8,
                },
                None,
            )?,
        ))
        .await?;
    assert_eq!(
        governed.breakers().await?.sample_interval_ns(),
        SAMPLE_INTERVAL,
        "admin_configure_circuit_breakers"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_add_circuit_breaker(PRICE_ID, BREAKER, breaker(), None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.breakers().contains_key(&BREAKER),
        "admin_add_circuit_breaker"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_set_manual_trip(PRICE_ID, true, Some(vec![0x01, 0x02]), None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.is_manually_tripped(),
        "admin_set_manual_trip"
    );

    governed
        .govern(Operation::TargetFunctionCall(target::admin_rearm(
            PRICE_ID,
            BREAKER,
            REARM_DELAY,
            None,
        )?))
        .await?;
    assert!(
        matches!(
            governed.breakers().await?.breakers()[&BREAKER].status,
            CircuitBreakerStatus::ArmedAfter { timestamp_ns }
                if timestamp_ns.as_ns() >= REARM_DELAY.as_ns()
        ),
        "admin_rearm"
    );

    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_enforced(
            PRICE_ID, BREAKER, false, None,
        )?))
        .await?;
    assert!(
        !governed.breakers().await?.breakers()[&BREAKER].is_enforced,
        "admin_set_enforced"
    );

    governed
        .govern(Operation::TargetFunctionCall(
            target::admin_remove_circuit_breaker(PRICE_ID, BREAKER, None)?,
        ))
        .await?;
    assert!(
        !governed.breakers().await?.breakers().contains_key(&BREAKER),
        "admin_remove_circuit_breaker"
    );

    // Last: clearing the proxy takes the circuit-breaker set with it.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None, "admin_set_proxy");
    Ok(())
}

/// `admin_upgrade` writes no readable state, and redeploying the current wasm leaves the code hash
/// unchanged — so it is proved by `contract_state` moving from local code to a global contract of
/// those same bytes. The released `0.3.0` would be the obvious different-bytes target, but its
/// `migrate` predates the empty-`migrate_args` no-op and rejects them.
#[rstest]
#[tokio::test]
async fn admin_upgrade_replaces_the_oracle_code(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;
    let before = code_hash(&governed.network, &governed.oracle).await?;
    let published = deploy_global_contract(
        &governed.network,
        &governed.admin,
        wasm::proxy_oracle().await.to_vec(),
    )
    .await?;

    governed
        .govern(Operation::TargetFunctionCall(target::admin_upgrade(
            UpgradeSource::GlobalHash(published),
            Base64VecU8(Vec::new()),
            None,
        )?))
        .await?;

    assert_ne!(
        before,
        code_hash(&governed.network, &governed.oracle).await?,
        "oracle code should have been replaced"
    );
    assert_eq!(
        view::<Option<AccountId>>(
            &governed.network,
            &governed.oracle,
            "own_get_owner",
            json!({})
        )
        .await?,
        Some(governed.governance.clone()),
        "the upgraded oracle should still be owned by its governance"
    );
    Ok(())
}

#[rstest]
#[tokio::test]
async fn borsh_entrypoint_drives_target_and_reflexive_operations(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern_borsh(Operation::TargetFunctionCall(
            target::admin_set_manual_trip(PRICE_ID, true, None, None)?,
        ))
        .await?;
    assert!(
        governed.breakers().await?.is_manually_tripped(),
        "borsh-created target call should have tripped the breaker"
    );

    let operator: AccountId = "operator.near".parse()?;
    governed
        .govern_borsh(Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: operator.clone(),
            role: Role::ManualTripper,
            set: true,
        }))
        .await?;
    assert!(
        view::<bool>(
            &governed.network,
            &governed.governance,
            "has_role",
            json!({ "account_id": operator, "role": "ManualTripper" }),
        )
        .await?,
        "borsh-created reflexive op should have granted the role"
    );
    Ok(())
}

/// The path every future governance upgrade takes, which `upgrade_ordering` cannot cover: the v0
/// contract has no `SelfUpgrade` and upgrades by full-access key instead. Both `UpgradeSource`
/// variants run here — identical redeployed bytes leave the code hash unchanged, so only the switch
/// to a global contract can show the deploy action landing.
#[rstest]
#[tokio::test]
async fn self_upgrade_redeploys_governance_and_keeps_it_working(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let mut governed = governed(&harness).await?;

    governed
        .govern(self_upgrade(UpgradeSource::Code(Base64VecU8(
            wasm::proxy_governance().await.to_vec(),
        ))))
        .await?;

    // Still governing after replacing its own code, which a redeployed hash cannot show.
    governed
        .govern(Operation::TargetFunctionCall(target::admin_set_proxy(
            PRICE_ID, None, None,
        )?))
        .await?;
    assert_eq!(governed.proxy().await?, None);

    let before = code_hash(&governed.network, &governed.governance).await?;
    let published = deploy_global_contract(
        &governed.network,
        &governed.admin,
        wasm::proxy_governance().await.to_vec(),
    )
    .await?;
    governed
        .govern(self_upgrade(UpgradeSource::GlobalHash(published)))
        .await?;
    assert_ne!(
        before,
        code_hash(&governed.network, &governed.governance).await?,
        "governance should now run the global contract, not its own local code"
    );
    Ok(())
}

/// Unreadable bodies must abort the whole revocation receipt, even after an earlier purge.
/// This exercises host rollback, not catch_unwind around a direct Rust call.
#[rstest]
#[case::missing(false)]
#[case::corrupt(true)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one receipt rollback scenario binds the corrupted fixture to a complete storage snapshot"
)]
async fn revocation_with_unreadable_body_rolls_back_membership_ledger_and_prior_purge(
    #[future(awt)] harness: SandboxHarness,
    #[case] corrupt: bool,
) -> Result<()> {
    use templar_gateway_methods_spec::account::ViewState;
    use templar_proxy_oracle_governance_kernel::Governance;
    use templar_proxy_oracle_near_governance_common::{CreateProposalArgs, GovernancePolicy};

    let mut governed = governed(&harness).await?;
    let delegate = harness.create_user("delegate").await?.0;
    governed
        .govern(Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: delegate.clone(),
            role: Role::Admin,
            set: true,
        }))
        .await?;
    common::call(
        &governed.network,
        &governed.governance,
        "create_proposal",
        CreateProposalArgs {
            id: 2,
            operation: Operation::TargetFunctionCall(target::admin_set_manual_trip(
                PRICE_ID, true, None, None,
            )?),
            requested_ttl: Nanoseconds::zero(),
        },
        &delegate,
        common::ONE_YOCTO,
    )
    .await?;
    common::call(
        &governed.network,
        &governed.governance,
        "create_proposal",
        CreateProposalArgs {
            id: 3,
            operation: Operation::Reflexive(ReflexiveOperation::SetRole {
                account_id: delegate.clone(),
                role: Role::Admin,
                set: false,
            }),
            requested_ttl: Nanoseconds::zero(),
        },
        &governed.admin,
        common::ONE_YOCTO,
    )
    .await?;

    let client = harness.client()?;
    let query = ViewState {
        account_id: governed.governance.clone(),
        prefix: Vec::new().into(),
    };
    let entries = client.read(query.clone()).await?;
    let state = entries
        .iter()
        .find(|entry| entry.key.0 == b"STATE")
        .expect("governance state exists");
    // Contract and VersionedState are transparent Borsh wrappers. LookupMap stores its prefix.
    // Check exact bytes before seeding a phantom active ID whose body is absent or undecodable.
    let (header, prefix, oracle): (Governance<GovernancePolicy>, Vec<u8>, AccountId) =
        near_sdk::borsh::from_slice(&state.value.0)?;
    assert_eq!(
        near_sdk::borsh::to_vec(&(header.clone(), prefix.clone(), oracle.clone()))?,
        state.value.0
    );
    assert_eq!(header.active_ids(), &[2, 3]);
    let header = Governance::try_from_parts(
        5,
        vec![2, 3, 4],
        header.ttls().clone(),
        header.max_pending_proposals(),
    )
    .unwrap();
    let mut patch = vec![(
        b"STATE".to_vec(),
        near_sdk::borsh::to_vec(&(header, prefix.clone(), oracle))?,
    )];
    if corrupt {
        let mut key = prefix;
        key.extend_from_slice(&4_u32.to_le_bytes());
        patch.push((key, vec![0xff]));
    }
    harness.patch_state(&governed.governance, patch).await?;
    let mut before = client.read(query.clone()).await?;
    let original = governed.proposal(2).await?;
    let revocation = governed.proposal(3).await?;
    let failed = governed
        .try_execute_with_gas(3, common::GATEWAY_GAS)
        .await?
        .assert_failure();
    let expected_error = if corrupt {
        "Cannot deserialize element"
    } else {
        "called `Option::unwrap_or_reject()` on a `None` value"
    };
    assert!(
        format!("{failed:?}").contains(expected_error),
        "revocation must reach the unreadable proposal body: {failed:?}"
    );
    assert!(
        failed
            .logs()
            .iter()
            .all(|log| !log.contains("\"event\":\"executed\"")),
        "failed revocation must not advertise successful execution"
    );
    assert!(
        view::<bool>(
            &governed.network,
            &governed.governance,
            "has_role",
            json!({ "account_id": delegate, "role": "Admin" }),
        )
        .await?
    );
    assert_eq!(governed.proposal(2).await?, original);
    assert_eq!(governed.proposal(3).await?, revocation);
    assert_eq!(
        view::<Vec<u32>>(
            &governed.network,
            &governed.governance,
            "list_proposals",
            json!({}),
        )
        .await?,
        vec![2, 3, 4]
    );
    let mut after = client.read(query).await?;
    before.sort_unstable_by(|a, b| a.key.0.cmp(&b.key.0));
    after.sort_unstable_by(|a, b| a.key.0.cmp(&b.key.0));
    assert_eq!(
        after, before,
        "failed receipt must revert every storage key"
    );
    Ok(())
}

/// A legacy (unaccounted) queue must still enforce policy recovery on execution,
/// and a failed execution must roll back the whole receipt rather than leaving
/// an unrecoverable policy key behind.
#[rstest]
#[tokio::test]
async fn legacy_unmarked_policy_execution_rolls_back_through_real_receipt(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    use near_sdk::Gas;
    use templar_gateway_methods_spec::account;
    use templar_gateway_testing::{wasm, TEST_FINALITY_POLICY};
    use templar_proxy_oracle_near_governance_common::{
        CreateProposalArgs, GovernancePolicy, LegacyOperation, MethodPolicy, Proposal,
    };

    let oracle = harness.create_user("legacy-oracle").await?;
    let governance = harness.create_user("legacy-governance").await?;
    let admin = harness.create_user("legacy-admin").await?;
    let small_legacy = LegacyOperation::SetManualTrip {
        id: PRICE_ID,
        is_manually_tripped: true,
        metadata: None,
    };
    let small_operation = Operation::try_from(small_legacy.clone())?;
    harness
        .deploy_and_init(
            &governance.0,
            wasm::released(
                templar_gateway_testing::ArtifactId::ProxyGovernance,
                "0.1.0",
            )
            .await,
            "new",
            json!({
                "proxy_oracle_id": oracle.0,
                "admin_id": admin.0,
                "ttls": {
                    "set_proxy": "0",
                    "configure_circuit_breakers": "0",
                    "add_circuit_breaker": "0",
                    "remove_circuit_breaker": "0",
                    "set_manual_trip": "0",
                    "rearm": "0",
                    "set_enforced": "0",
                    "set_action_ttl": "0",
                    "set_role": "0",
                    "admin_upgrade": "0",
                    "admin_function_call": "0",
                },
            }),
        )
        .await?;
    common::call(
        &harness.network,
        &governance.0,
        "create_proposal",
        CreateProposalArgs {
            id: 0,
            operation: small_legacy,
            requested_ttl: Nanoseconds::zero(),
        },
        &admin.0,
        common::ONE_YOCTO,
    )
    .await?;
    // Migrating a real released contract produces an unmarked legacy queue;
    // no byte accounting exists until it drains.
    harness
        .deploy_and_init(
            &governance.0,
            wasm::proxy_governance().await.to_vec(),
            "migrate",
            json!({"from_version": "v0"}),
        )
        .await?;

    let method = "m".repeat(1_300_000);
    let long_proposal = Proposal {
        operation: Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: method.clone(),
            policy: Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::Admin,
            }),
        }),
        created_at: Nanoseconds::zero(),
        ttl: Nanoseconds::zero(),
        created_by: admin.0.clone(),
    };
    let client = harness.client()?;
    let entries = client
        .read(account::ViewState {
            account_id: governance.0.clone(),
            prefix: Vec::new().into(),
        })
        .await?;
    let legacy_entries = entries
        .into_iter()
        .map(|entry| {
            let value = near_sdk::borsh::from_slice::<Proposal<Operation>>(&entry.value.0)
                .ok()
                .filter(|proposal| proposal.operation == small_operation)
                .map_or(entry.value.0, |_| {
                    near_sdk::borsh::to_vec(&long_proposal).expect("proposal serializes")
                });
            (entry.key.0, value)
        })
        .collect::<Vec<_>>();
    harness.patch_state(&governance.0, legacy_entries).await?;
    let before = {
        let mut entries = client
            .read(account::ViewState {
                account_id: governance.0.clone(),
                prefix: Vec::new().into(),
            })
            .await?;
        entries.sort_by(|a, b| a.key.0.cmp(&b.key.0));
        entries
    };

    let error = near_api::Contract(governance.0.clone())
        .call_function("execute_proposal", json!({"id": 0}))
        .transaction()
        .deposit(common::ONE_YOCTO)
        .gas(Gas::from_tgas(300))
        .with_signer(admin.0.clone(), common::signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(&harness.network)
        .await?
        .assert_failure();
    assert!(
        format!("{error:?}").contains("Governance policy exceeds recoverable working set"),
        "unexpected failure: {error:?}"
    );
    assert_eq!(
        view::<Option<Proposal<Operation>>>(
            &harness.network,
            &governance.0,
            "get_proposal",
            json!({"id": 0}),
        )
        .await?,
        Some(long_proposal)
    );
    let policy: GovernancePolicy = view(
        &harness.network,
        &governance.0,
        "get_governance_policy",
        json!({}),
    )
    .await?;
    assert!(!policy.method_policies().contains_key(&method));
    let mut after = client
        .read(account::ViewState {
            account_id: governance.0.clone(),
            prefix: Vec::new().into(),
        })
        .await?;
    after.sort_by(|a, b| a.key.0.cmp(&b.key.0));
    assert_eq!(
        after, before,
        "failed receipt must revert every storage key"
    );
    Ok(())
}
