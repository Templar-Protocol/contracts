//! Shared fixtures for the gateway-`SandboxHarness`-based proxy-oracle
//! integration tests.
//!
//! Contract calls go through the harness's in-process gateway client, so these
//! tests exercise the same dispatch the RPC service uses. What lives here is
//! only what the gateway cannot supply: price payloads stamped against *chain*
//! time, and the raw-state fixtures the migration tests replay.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use anyhow::Result;
use near_sdk::json_types::{I64, U64};
use templar_common::{
    oracle::{
        pyth::{self, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types::U256,
    Nanoseconds,
};
use templar_gateway_testing::SandboxHarness;

/// A Pyth price at the current *chain* time, expo `0`. Chain time, not the host
/// clock — see [`SandboxHarness::chain_timestamp`].
pub async fn pyth_price_now(harness: &SandboxHarness, value: i64) -> Result<pyth::Price> {
    Ok(pyth_price_at(value, harness.chain_timestamp().await?))
}

/// A Pyth price stamped at an explicit time, expo `0`.
pub fn pyth_price_at(value: i64, time: Nanoseconds) -> pyth::Price {
    pyth::Price {
        price: I64(value),
        conf: U64(0),
        expo: 0,
        publish_time: PythTimestamp::try_from_time(time).unwrap(),
    }
}

/// A RedStone feed (8-decimal) at the current *chain* time. Chain time, not the
/// host clock — see [`SandboxHarness::chain_timestamp`].
pub async fn redstone_price_now(harness: &SandboxHarness, value: u128) -> Result<FeedData> {
    Ok(redstone_price_at(value, harness.chain_timestamp().await?))
}

/// A RedStone feed (8-decimal) stamped at an explicit time.
pub fn redstone_price_at(value: u128, time: Nanoseconds) -> FeedData {
    FeedData {
        price: U256::from(value * 100_000_000_u128).into(),
        package_timestamp: time,
        write_timestamp: time,
    }
}

/// Raw contract state: storage key -> value, as captured for migration fixtures.
pub type StatePatch = std::collections::HashMap<Vec<u8>, Vec<u8>>;

/// Reproduce a pre-kernelization (v0) proxy oracle, then migrate its code.
///
/// Deploys the legacy `0.1.0` wasm to the harness proxy-oracle account, patches
/// the supplied raw v0 state onto it, then redeploys the current wasm over it
/// without an init call (leaving the stored state at v0 so migration is
/// exercised). Returns the contract account id.
pub async fn deploy_from_patch(
    harness: &SandboxHarness,
    patch: StatePatch,
) -> Result<near_api::types::AccountId> {
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::released(
                templar_gateway_testing::ArtifactId::ProxyOracle,
                "0.1.0",
            )
            .await,
        )
        .await?;

    harness.patch_state(&account_id, patch).await?;

    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::proxy_oracle().await.to_vec(),
        )
        .await?;

    Ok(account_id)
}
