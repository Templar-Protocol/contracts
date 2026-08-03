//! End-to-end proof that upgrading the proxy oracle and its governance contract from their
//! currently-deployed (pre-standardized-upgrade) wasm to this branch's wasm leaves both contracts
//! functional, **in either order**.
//!
//! The deployed contracts predate the standardized upgrade path, so the upgrades are key-driven: a
//! bare `DeployContract` for the v1 oracle (state layout unchanged, no migrate) and
//! `DeployContract + migrate` for the gov (v0 → v1). Because each account upgrades independently
//! through its own key, neither order strands the other — the ordering coupling only bites once the
//! governance-driven cross-contract path is in use, which requires both sides already upgraded.
//!
//! The gov starts with a pending `AdminUpgrade` proposal, so the v0→v1 migrate must reshape a real
//! stored proposal body on-chain (`AdminUpgrade`'s raw `code` → `UpgradeSource::Code`), not just an
//! empty map; each upgraded contract is then probed with a domain view to prove it still answers,
//! not merely that its code hash changed.
//!
//! Fixtures are the real on-chain blobs (proxy-oracle `0.3.0`, state v1;
//! proxy-governance `0.1.0`, pre-versioned-state), pinned from mainnet and
//! catalogued as releases in `contract/artifacts/releases/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_api::NetworkConfig;
use near_sdk::json_types::Base64VecU8;
use near_sdk::NearToken;
use serde_json::{json, Value};
use templar_common::upgrade::UpgradeSource;
use templar_common::Nanoseconds;
use templar_gateway_testing::{wasm, ArtifactId, SandboxHarness};
use templar_proxy_oracle_near_governance_common::{LegacyOperation, Operation, Proposal};

use common::{
    call, code_hash, deploy_code, deploy_with_init, view, CreateProposalArgs, ProposalIdArgs,
};

/// The raw blob (`0xDEADBEEF`) of the pending `AdminUpgrade` proposal seeded on the old gov, whose
/// stored body the v0→v1 migrate must reshape into a generic `admin_upgrade` target call.
const PENDING_UPGRADE_CODE: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

/// The `AdminUpgrade` seeded before migration, in the pre-restructure typed form. Serializing it for
/// the old gov also guards that the wire shape stays compatible with the immutable deployed contract.
fn seeded_upgrade() -> LegacyOperation {
    LegacyOperation::AdminUpgrade {
        code: UpgradeSource::Code(Base64VecU8(PENDING_UPGRADE_CODE.to_vec())),
        migrate_args: Base64VecU8(Vec::new()),
    }
}

/// The pre-standardized-upgrade (v0) 11-field `TtlConfig` — no `self_upgrade`.
fn old_ttls() -> Value {
    json!({
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
    })
}

/// An OLD oracle (owned by the gov account) governed by an OLD gov. Returns `(oracle, gov)`.
async fn setup(harness: &SandboxHarness) -> Result<(AccountId, AccountId)> {
    let oracle = harness.create_user("oracle").await?.0;
    let gov = harness.create_user("gov").await?.0;
    let admin = harness.create_user("admin").await?.0;
    let network = &harness.network;

    // The oracle is owned by the gov account, so the gov can drive `admin_upgrade`.
    tokio::try_join!(
        deploy_with_init(
            network,
            &oracle,
            wasm::released(ArtifactId::ProxyOracle, "0.3.0").await,
            "new",
            json!({ "owner_id": gov }),
        ),
        deploy_with_init(
            network,
            &gov,
            wasm::released(ArtifactId::ProxyGovernance, "0.1.0").await,
            "new",
            json!({ "proxy_oracle_id": oracle, "admin_id": admin, "ttls": old_ttls() }),
        ),
    )?;

    // Seed a pending AdminUpgrade proposal on the OLD gov (v0 `code` is a raw blob) so the later
    // v0→v1 migrate must reshape a real stored proposal body, not an empty map.
    call(
        network,
        &gov,
        "create_proposal",
        CreateProposalArgs {
            id: 0,
            operation: seeded_upgrade(),
            requested_ttl: Nanoseconds::zero(),
        },
        &admin,
        NearToken::from_yoctonear(1),
    )
    .await?;

    Ok((oracle, gov))
}

/// Upgrade the oracle via its key: a bare deploy (unchanged v1 state layout, so no migrate). Asserts
/// the code was replaced and the contract stays functional — it reports state version 1 and still
/// answers a domain view (`own_get_owner`) with its pre-upgrade owner.
async fn upgrade_oracle(
    network: &NetworkConfig,
    oracle: &AccountId,
    owner: &AccountId,
) -> Result<()> {
    let before = code_hash(network, oracle).await?;
    deploy_code(network, oracle, wasm::proxy_oracle().await.to_vec()).await?;
    assert_ne!(
        before,
        code_hash(network, oracle).await?,
        "oracle code should have been replaced"
    );
    assert_eq!(
        view::<u32>(network, oracle, "get_stored_state_version", json!({})).await?,
        1
    );
    assert_eq!(
        view::<Option<AccountId>>(network, oracle, "own_get_owner", json!({})).await?,
        Some(owner.clone()),
        "upgraded oracle should retain its owner"
    );
    Ok(())
}

/// Upgrade the gov via its key: deploy + v0→v1 migrate. Asserts the code was replaced and the gov is
/// now versioned (`get_stored_state_version` — a method the old, pre-versioned-state gov lacked).
async fn upgrade_gov(network: &NetworkConfig, gov: &AccountId) -> Result<()> {
    let before = code_hash(network, gov).await?;
    deploy_with_init(
        network,
        gov,
        wasm::proxy_governance().await.to_vec(),
        "migrate",
        json!({ "from_version": "v0" }),
    )
    .await?;
    assert_ne!(
        before,
        code_hash(network, gov).await?,
        "gov code should have been replaced"
    );
    assert_eq!(
        view::<u32>(network, gov, "get_stored_state_version", json!({})).await?,
        1
    );
    // The seeded proposal survived the v0→v1 borsh reshape: it still deserializes as a v1
    // `Proposal<Operation>` from storage, and its raw `code` became `UpgradeSource::Code` intact.
    let proposal: Option<Proposal<Operation>> =
        view(network, gov, "get_proposal", ProposalIdArgs { id: 0 }).await?;
    let operation = proposal
        .expect("seeded proposal survived migration")
        .operation;
    assert_eq!(
        operation,
        Operation::try_from(seeded_upgrade()).unwrap(),
        "migrated proposal should reshape the raw code into an admin_upgrade target call"
    );
    Ok(())
}

#[tokio::test]
async fn oracle_first_upgrade_does_not_brick() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = &harness.network;
    let (oracle, gov) = setup(&harness).await?;

    upgrade_oracle(network, &oracle, &gov).await?;
    upgrade_gov(network, &gov).await?;

    Ok(())
}

#[tokio::test]
async fn gov_first_upgrade_does_not_brick() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = &harness.network;
    let (oracle, gov) = setup(&harness).await?;

    upgrade_gov(network, &gov).await?;
    upgrade_oracle(network, &oracle, &gov).await?;

    Ok(())
}
