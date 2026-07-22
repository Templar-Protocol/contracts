//! Minimal `near_api` helpers for the sandbox upgrade tests, mirroring the proxy-oracle test common.
//! Every harness account shares the same well-known test key, so one [`signer`] signs for any of
//! them; reads and writes pin the shared [`TEST_FINALITY_POLICY`] for deterministic finality.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use near_api::types::AccountId;
use near_api::{Account, Contract, NetworkConfig};
use near_sdk::serde::{de::DeserializeOwned, Serialize};

pub use templar_gateway_testing::test_signer as signer;
use templar_gateway_testing::TEST_FINALITY_POLICY;

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
        .at(TEST_FINALITY_POLICY.query_reference())
        .fetch_from(network)
        .await?
        .data)
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
        .with_signer(signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Submit a mutating call to `contract_id` signed as `signer_id` (all harness accounts share the
/// test key), attaching `deposit`.
pub async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<()> {
    Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .deposit(deposit)
        .gas(near_sdk::Gas::from_tgas(100))
        .with_signer(signer_id.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
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
            .at(TEST_FINALITY_POLICY.query_reference())
            .fetch_from(network)
            .await?
            .data
            .contract_state
    ))
}
