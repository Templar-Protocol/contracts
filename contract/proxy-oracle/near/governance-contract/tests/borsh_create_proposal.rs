//! `create_proposal_borsh` against a real node: parity, gas, and payload size.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_api::NetworkConfig;
use near_sdk::json_types::Base64VecU8;
use rstest::rstest;
use serde_json::json;
use templar_common::{upgrade::UpgradeSource, Nanoseconds};
use templar_gateway_testing::{harness, wasm, SandboxHarness};
use templar_proxy_oracle_near_governance_common::{GovernancePolicy, Operation, Proposal};

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
