//! End-to-end proof that upgrading the proxy oracle and its governance contract from their
//! currently-deployed (pre-standardized-upgrade) wasm to this branch's wasm does **not** brick,
//! **in either order**.
//!
//! Upgrades are key-driven (the deployed contracts can't be changed): a bare `DeployContract` for
//! the v1 oracle (state layout unchanged, no migrate) and `DeployContract + migrate` for the gov
//! (v0 → v1). Each upgrades independently through its own key, so neither order strands the other.
//!
//! Fixtures are the real on-chain blobs (`PROXY_ORACLE_0_3_0`, state v1; `PROXY_GOVERNANCE_0_1_0`,
//! pre-versioned-state), pinned from mainnet.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use anyhow::Result;
use near_api::types::AccountId;
use near_api::{Contract, NetworkConfig};
use serde_json::{json, Value};
use templar_gateway_testing::{wasm, SandboxHarness, TEST_FINALITY_POLICY};

use common::{code_hash, deploy_code, signer, view};

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

/// Deploy `code` to `account` via its full-access key, atomically calling `method` (`new` to init,
/// `migrate` to bootstrap-upgrade).
async fn deploy_with_init(
    network: &NetworkConfig,
    account: &AccountId,
    code: Vec<u8>,
    method: &str,
    args: Value,
) -> Result<()> {
    Contract::deploy(account.clone())
        .use_code(code)
        .with_init_call(method, args)?
        .max_gas()
        .with_signer(signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
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
            wasm::PROXY_ORACLE_0_3_0.to_vec(),
            "new",
            json!({ "owner_id": gov }),
        ),
        deploy_with_init(
            network,
            &gov,
            wasm::PROXY_GOVERNANCE_0_1_0.to_vec(),
            "new",
            json!({ "proxy_oracle_id": oracle, "admin_id": admin, "ttls": old_ttls() }),
        ),
    )?;

    Ok((oracle, gov))
}

/// Upgrade the oracle via its key: a bare deploy (unchanged v1 state layout, so no migrate). Asserts
/// the code was replaced and the contract stays functional (answers a versioned-state view at v1).
async fn upgrade_oracle(network: &NetworkConfig, oracle: &AccountId) -> Result<()> {
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
    Ok(())
}

#[tokio::test]
async fn oracle_first_upgrade_does_not_brick() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = &harness.network;
    let (oracle, gov) = setup(&harness).await?;

    upgrade_oracle(network, &oracle).await?;
    upgrade_gov(network, &gov).await?;

    Ok(())
}

#[tokio::test]
async fn gov_first_upgrade_does_not_brick() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = &harness.network;
    let (oracle, gov) = setup(&harness).await?;

    upgrade_gov(network, &gov).await?;
    upgrade_oracle(network, &oracle).await?;

    Ok(())
}
