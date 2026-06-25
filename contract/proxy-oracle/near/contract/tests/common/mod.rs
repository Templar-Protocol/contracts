//! Shared helpers for the gateway-`SandboxHarness`-based proxy-oracle
//! integration tests.
//!
//! These tests drive the proxy-oracle contract over the sandbox the same way
//! the harness itself does: typed `near_api` view calls for reads, and signed
//! `near_api` transactions for writes. Every sandbox account the harness creates
//! shares the same well-known test key, so a single [`signer`] can sign for any
//! of them (the contract account, mock oracles, ad-hoc users, ...).
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use anyhow::{Context, Result};
use near_api::{Contract, NetworkConfig, SecretKey, Signer};
use near_sdk::serde::{de::DeserializeOwned, Serialize};
use near_sdk::{
    json_types::{I64, U64},
    Gas,
};
use near_token::NearToken;
use templar_common::{
    oracle::{
        pyth::{self, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types::U256,
    Nanoseconds,
};

/// The fixed sandbox key shared by every account the harness provisions.
pub const TEST_SECRET_KEY: &str =
    "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

/// Parse the shared sandbox secret key.
pub fn secret_key() -> Result<SecretKey> {
    TEST_SECRET_KEY
        .parse()
        .context("failed to parse test secret key")
}

/// Build a signer over the shared sandbox key. Valid for any harness account.
pub fn signer() -> Result<Arc<Signer>> {
    Signer::from_secret_key(secret_key()?).context("failed to build test signer")
}

/// Create a fresh, signable sandbox account under the shared test key.
pub async fn create_account(
    harness: &templar_gateway_testing::SandboxHarness,
    label: &str,
) -> Result<near_api::types::AccountId> {
    // Harness accounts are all created with the shared test key, so the global
    // `signer()` signs for the returned account. The exact id is generated.
    Ok(harness.create_user(label).await?.0)
}

/// Dispatch a contract view call and deserialize the result.
pub async fn view<T: DeserializeOwned + Send + Sync>(
    network: &NetworkConfig,
    contract_id: &near_api::types::AccountId,
    method: &str,
    args: impl Serialize,
) -> Result<T> {
    Ok(Contract(contract_id.clone())
        .call_function(method, args)
        .read_only::<T>()
        .fetch_from(network)
        .await?
        .data)
}

/// Submit a signed contract call and assert it succeeded.
pub async fn call(
    network: &NetworkConfig,
    contract_id: &near_api::types::AccountId,
    signer_id: &near_api::types::AccountId,
    method: &str,
    args: impl Serialize,
    gas_tgas: u64,
    deposit_yocto: u128,
) -> Result<()> {
    Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .gas(Gas::from_tgas(gas_tgas))
        .deposit(NearToken::from_yoctonear(deposit_yocto))
        .with_signer(signer_id.clone(), signer()?)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Submit a signed contract call without asserting success, returning the raw
/// transaction result so the caller can assert on a failure message.
pub async fn try_call(
    network: &NetworkConfig,
    contract_id: &near_api::types::AccountId,
    signer_id: &near_api::types::AccountId,
    method: &str,
    args: impl Serialize,
    gas_tgas: u64,
    deposit_yocto: u128,
) -> Result<near_api::types::transaction::result::TransactionResult> {
    Ok(Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .gas(Gas::from_tgas(gas_tgas))
        .deposit(NearToken::from_yoctonear(deposit_yocto))
        .with_signer(signer_id.clone(), signer()?)
        .send_to(network)
        .await?)
}

/// Assert a transaction failed and its failure debug contains `needle`.
pub fn assert_failure_contains(
    result: near_api::types::transaction::result::TransactionResult,
    needle: &str,
) {
    assert!(
        result.is_failure(),
        "expected transaction to fail with {needle:?}, but it succeeded"
    );
    let error = result
        .into_result()
        .expect_err("expected a failed transaction");
    let message = format!("{error:?}");
    assert!(
        message.contains(needle),
        "expected failure containing {needle:?}, got: {message}"
    );
}

/// A Pyth price at the current wall-clock time, expo `0`.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn pyth_price_now(value: i64) -> pyth::Price {
    pyth::Price {
        price: I64(value),
        conf: U64(0),
        expo: 0,
        publish_time: PythTimestamp::from_secs(
            std::time::UNIX_EPOCH
                .elapsed()
                .unwrap_or_default()
                .as_secs() as i64,
        ),
    }
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

/// A RedStone feed (8-decimal) at the current wall-clock time.
#[allow(clippy::cast_possible_truncation)]
pub fn redstone_price_now(value: u128) -> FeedData {
    let now = Nanoseconds::from_ms(
        std::time::UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_millis() as u64,
    );
    redstone_price_at(value, now)
}

/// A RedStone feed (8-decimal) stamped at an explicit time.
pub fn redstone_price_at(value: u128, time: Nanoseconds) -> FeedData {
    FeedData {
        price: U256::from(value * 100_000_000_u128).into(),
        package_timestamp: time,
        write_timestamp: time,
    }
}

/// Current wall-clock as [`Nanoseconds`].
#[allow(clippy::cast_possible_truncation)]
pub fn now_ns() -> Nanoseconds {
    Nanoseconds::from_ms(
        std::time::UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_millis() as u64,
    )
}

/// Raw contract state: storage key -> value, as captured for migration fixtures.
pub type StatePatch = std::collections::HashMap<Vec<u8>, Vec<u8>>;

/// Deploy raw wasm to `account_id` with no init call.
async fn deploy_code(
    network: &NetworkConfig,
    account_id: &near_api::types::AccountId,
    code: Vec<u8>,
) -> Result<()> {
    Contract::deploy(account_id.clone())
        .use_code(code)
        .without_init_call()
        .with_signer(signer()?)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Reproduce a pre-kernelization (v0) proxy oracle, then migrate its code.
///
/// Deploys the legacy `0.1.0` wasm to the harness proxy-oracle account, patches
/// the supplied raw v0 state onto it, then redeploys the current wasm over it
/// without an init call (leaving the stored state at v0 so migration is
/// exercised). Returns the contract account id.
pub async fn deploy_from_patch(
    harness: &templar_gateway_testing::SandboxHarness,
    patch: StatePatch,
) -> Result<near_api::types::AccountId> {
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    deploy_code(
        &harness.network,
        &account_id,
        test_utils::ProxyOracleController::wasm_v0().to_vec(),
    )
    .await?;

    harness.patch_state(&account_id, patch).await?;

    deploy_code(
        &harness.network,
        &account_id,
        test_utils::ProxyOracleController::wasm().await.to_vec(),
    )
    .await?;

    Ok(account_id)
}
