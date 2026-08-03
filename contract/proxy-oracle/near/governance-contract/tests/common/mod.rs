//! Minimal `near_api` helpers for the sandbox upgrade tests, mirroring the proxy-oracle test common.
//! Every harness account shares the same well-known test key, so one [`signer`] signs for any of
//! them; reads and writes pin the shared [`TEST_FINALITY_POLICY`] for deterministic finality.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use near_api::types::transaction::result::ExecutionSuccess;
use near_api::types::AccountId;
use near_api::{Account, Contract, NetworkConfig};
use near_sdk::json_types::Base58CryptoHash;
use near_sdk::serde::{de::DeserializeOwned, Serialize};
use templar_proxy_oracle_near_governance_common::{Operation, ReflexiveOperation};

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

/// Signed by `account_id`, which pays: a global deploy costs ten times a normal one, permanently.
pub async fn deploy_global_contract(
    network: &NetworkConfig,
    account_id: &AccountId,
    code: Vec<u8>,
) -> Result<Base58CryptoHash> {
    let hash = near_api::types::CryptoHash::hash(&code).0;
    Contract::deploy_global_contract_code(code)
        .as_hash()
        .with_signer(account_id.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(Base58CryptoHash::from(hash))
}

/// Deploy `code` to `account` via its full-access key, atomically calling `method` (`new` to init,
/// `migrate` to bootstrap-upgrade).
pub async fn deploy_with_init(
    network: &NetworkConfig,
    account: &AccountId,
    code: Vec<u8>,
    method: &str,
    args: near_sdk::serde_json::Value,
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
///
/// 300 Tgas, not the protocol's 1000, because that is what the gateway attaches
/// (`proxy_oracle_governance_impl`) and so what an `admin_upgrade` forwarding 280 Tgas has to fit in.
pub async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<ExecutionSuccess> {
    let args = near_sdk::serde_json::to_vec(&args)?;
    call_raw(network, contract_id, method, args, signer_id, deposit).await
}

pub async fn call_borsh(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl near_sdk::borsh::BorshSerialize,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<ExecutionSuccess> {
    let args = near_sdk::borsh::to_vec(&args)?;
    call_raw(network, contract_id, method, args, signer_id, deposit).await
}

async fn call_raw(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: Vec<u8>,
    signer_id: &AccountId,
    deposit: near_sdk::NearToken,
) -> Result<ExecutionSuccess> {
    let outcome = Contract(contract_id.clone())
        .call_function_raw(method, args)
        .transaction()
        .deposit(deposit)
        .gas(near_sdk::Gas::from_tgas(300))
        .with_signer(signer_id.clone(), signer())
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();

    // Governance dispatches its effects on detached promises, so a failed target call still leaves
    // the top-level status successful.
    let failures = outcome.receipt_failures();
    assert!(
        failures.is_empty(),
        "{method} left failed receipts: {failures:#?}"
    );
    Ok(outcome)
}

pub use templar_proxy_oracle_near_governance_common::CreateProposalArgs;

pub const ONE_YOCTO: near_sdk::NearToken = near_sdk::NearToken::from_yoctonear(1);

/// A `SelfUpgrade` with nothing for `migrate` to do — the state version is already current.
pub fn self_upgrade(code: templar_common::upgrade::UpgradeSource) -> Operation {
    Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code,
        migrate_args: near_sdk::json_types::Base64VecU8(Vec::new()),
    })
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ProposalIdArgs {
    pub id: u32,
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
