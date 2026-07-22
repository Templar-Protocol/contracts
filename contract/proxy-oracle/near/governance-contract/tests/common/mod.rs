//! Minimal `near_api` helpers for the sandbox upgrade tests. Every harness account shares the same
//! well-known test key, so one [`signer`] signs for any of them.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use anyhow::{Context, Result};
use near_api::types::AccountId;
use near_api::{Account, Contract, NetworkConfig, Signer};
use near_sdk::serde::{de::DeserializeOwned, Serialize};
use near_sdk::Gas;
use near_token::NearToken;

/// A signer over the shared sandbox key, valid for any harness account.
pub fn signer() -> Result<Arc<Signer>> {
    Signer::from_secret_key(templar_gateway_testing::test_secret_key()?)
        .context("failed to build test signer")
}

/// Dispatch a view call and deserialize the result.
pub async fn view<T: DeserializeOwned + Send + Sync>(
    network: &NetworkConfig,
    contract_id: &AccountId,
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

/// Submit a signed contract call and assert it succeeded (top-level).
pub async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    signer_id: &AccountId,
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

/// Deploy raw wasm to `account` via its full-access key, with no init call (a bare code refresh).
pub async fn deploy_code(
    network: &NetworkConfig,
    account: &AccountId,
    code: Vec<u8>,
) -> Result<()> {
    Contract::deploy(account.clone())
        .use_code(code)
        .without_init_call()
        .with_signer(signer()?)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// The account's deployed code hash, as a stable string for equality checks (a change proves the
/// code was actually replaced — i.e. the upgrade took effect and didn't revert).
pub async fn code_hash(network: &NetworkConfig, id: &AccountId) -> Result<String> {
    // `contract_state` is `ContractState::LocalHash(<hash>)` for a normally-deployed contract.
    Ok(format!(
        "{:?}",
        Account(id.clone())
            .view()
            .fetch_from(network)
            .await?
            .data
            .contract_state
    ))
}
