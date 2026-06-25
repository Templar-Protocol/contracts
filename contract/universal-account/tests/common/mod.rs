//! Shared harness helpers for the universal-account integration tests.
//!
//! These tests drive a real universal-account contract on a `near-sandbox`
//! node provisioned by the gateway [`SandboxHarness`]. Rather than going
//! through the gateway `Client` (whose universal-account write/read surface is
//! still being built out on this branch), we drive the contract directly with
//! raw `near_api` calls — the same `tx::FunctionCall`-style path the harness
//! itself uses for `set_mock_oracle_pyth_price` and friends. This keeps the
//! tests self-contained and requires no edits to `gateway/testing`.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use anyhow::Result;
use near_api::{AccountId, Contract, NetworkConfig, SecretKey, Signer};
use near_sdk::{json_types::U128, Gas};
use near_token::NearToken;
use serde::Serialize;
use serde_json::json;
use templar_gateway_testing::SandboxHarness;
use templar_universal_account::{
    transaction::FunctionCallAction, ExecuteArgs, KeyId, PayloadExecutionParameters,
};

/// The fixed secret key every harness-provisioned account is created with (see
/// `gateway/testing/src/sandbox.rs`). Newly created relayer accounts reuse it.
const TEST_SECRET_KEY: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

/// rstest fixture: a freshly started sandbox harness for a single test.
#[rstest::fixture]
pub async fn harness() -> SandboxHarness {
    SandboxHarness::start()
        .await
        .expect("failed to start sandbox harness")
}

pub fn test_secret_key() -> SecretKey {
    TEST_SECRET_KEY.parse().expect("valid test secret key")
}

pub fn test_signer() -> Arc<Signer> {
    Signer::from_secret_key(test_secret_key()).expect("valid signer")
}

/// The universal-account contract account provisioned by the harness.
pub fn ua_id(harness: &SandboxHarness) -> AccountId {
    harness.universal_account_signer_account_id.0.clone()
}

/// The mock fungible-token contract provisioned (and `new`-initialized) by the
/// harness.
pub fn ft_id(harness: &SandboxHarness) -> AccountId {
    harness.ft_contract_id.clone()
}

/// Convert a `near_api` account id into the `near_sdk` account id used by the
/// universal-account types.
pub fn to_sdk(id: &AccountId) -> near_sdk::AccountId {
    id.to_string().parse().expect("valid account id")
}

/// Create a fresh harness account (a `*.<tenant-root>` sub-account, created with
/// the shared test key) and return its id and signer. Works in both attach and
/// owned mode. The `label` only seeds a unique id — the exact id is generated.
pub async fn create_account(
    harness: &SandboxHarness,
    label: &str,
) -> Result<(AccountId, Arc<Signer>)> {
    let account_id = harness.create_user(label).await?.0;
    Ok((account_id, test_signer()))
}

/// Deploy `code` to `account_id` without an init call (used both for legacy
/// wasms before state-patching and for redeploying the current wasm on top).
pub async fn deploy_code(
    network: &NetworkConfig,
    account_id: &AccountId,
    signer: Arc<Signer>,
    code: Vec<u8>,
) -> Result<()> {
    Contract::deploy(account_id.clone())
        .use_code(code)
        .without_init_call()
        .with_signer(signer)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Deploy `code` to `account_id` and run its `method` init call.
pub async fn deploy_with_init(
    network: &NetworkConfig,
    account_id: &AccountId,
    signer: Arc<Signer>,
    code: Vec<u8>,
    method: &str,
    args: impl Serialize,
) -> Result<()> {
    Contract::deploy(account_id.clone())
        .use_code(code)
        .with_init_call(method, args)?
        .with_signer(signer)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Outcome of a state-changing call. Captures whether it succeeded plus a
/// rendering of any on-chain failures (mirrors the panic message that
/// `near-workspaces` `should_panic` assertions used to match against).
pub struct CallOutcome {
    pub success: bool,
    pub failures: String,
}

impl CallOutcome {
    pub fn assert_success(&self) {
        assert!(
            self.success,
            "expected call to succeed, got: {}",
            self.failures
        );
    }

    pub fn assert_failure_contains(&self, expected: &str) {
        assert!(
            !self.success,
            "expected call to fail with {expected:?}, but it succeeded"
        );
        assert!(
            self.failures.contains(expected),
            "expected failure to contain {expected:?}, got: {}",
            self.failures
        );
    }
}

/// Send a state-changing function call, returning its [`CallOutcome`] rather
/// than asserting, so callers can assert success *or* an expected failure.
#[allow(clippy::too_many_arguments)]
pub async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
    deposit: NearToken,
    gas: Gas,
    signer_id: &AccountId,
    signer: Arc<Signer>,
) -> Result<CallOutcome> {
    let result = Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .deposit(deposit)
        .gas(gas)
        .with_signer(signer_id.clone(), signer)
        .send_to(network)
        .await?;

    let success = result.is_success();
    let failures = result.into_full().map_or_else(
        || "<transaction still pending>".to_string(),
        |full| format!("{:#?}", full.failures()),
    );

    Ok(CallOutcome { success, failures })
}

/// Relay a signed universal-account payload through the contract's `execute`
/// method, signed by `relayer_id`.
pub async fn execute_as(
    network: &NetworkConfig,
    ua: &AccountId,
    relayer_id: &AccountId,
    relayer_signer: Arc<Signer>,
    args: ExecuteArgs<Box<[templar_universal_account::transaction::Transaction]>>,
) -> Result<CallOutcome> {
    call(
        network,
        ua,
        "execute",
        json!({ "args": args }),
        NearToken::from_near(0),
        Gas::from_tgas(300),
        relayer_id,
        relayer_signer,
    )
    .await
}

/// Call `migrate` reflexively (signed by the universal-account itself).
pub async fn migrate(
    network: &NetworkConfig,
    ua: &AccountId,
    args: impl Serialize,
) -> Result<CallOutcome> {
    call(
        network,
        ua,
        "migrate",
        args,
        NearToken::from_near(0),
        Gas::from_tgas(300),
        ua,
        test_signer(),
    )
    .await
}

/// Register `account_id` for storage on the FT so it can receive minted tokens.
pub async fn ft_storage_deposit(
    network: &NetworkConfig,
    ft: &AccountId,
    account_id: &AccountId,
    signer_id: &AccountId,
    signer: Arc<Signer>,
) -> Result<()> {
    call(
        network,
        ft,
        "storage_deposit",
        json!({ "account_id": account_id, "registration_only": true }),
        NearToken::from_yoctonear(NearToken::from_near(1).as_yoctonear() / 4),
        Gas::from_tgas(30),
        signer_id,
        signer,
    )
    .await?
    .assert_success();
    Ok(())
}

pub async fn ft_balance_of(
    network: &NetworkConfig,
    ft: &AccountId,
    account_id: &AccountId,
) -> Result<u128> {
    let balance: U128 = Contract(ft.clone())
        .call_function("ft_balance_of", json!({ "account_id": account_id }))
        .read_only()
        .fetch_from(network)
        .await?
        .data;
    Ok(balance.0)
}

pub async fn get_counter(
    network: &NetworkConfig,
    ft: &AccountId,
    account_id: &AccountId,
) -> Result<u32> {
    Ok(Contract(ft.clone())
        .call_function("get_counter", json!({ "account_id": account_id }))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

pub async fn get_key(
    network: &NetworkConfig,
    ua: &AccountId,
    key: &KeyId,
) -> Result<Option<PayloadExecutionParameters>> {
    Ok(Contract(ua.clone())
        .call_function("get_key", json!({ "key": key }))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

pub async fn list_keys(network: &NetworkConfig, ua: &AccountId) -> Result<Vec<KeyId>> {
    Ok(Contract(ua.clone())
        .call_function("list_keys", json!({ "offset": null, "count": null }))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

pub async fn stored_state_version(network: &NetworkConfig, ua: &AccountId) -> Result<u32> {
    Ok(Contract(ua.clone())
        .call_function("get_stored_state_version", json!({}))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

pub async fn target_state_version(network: &NetworkConfig, ua: &AccountId) -> Result<u32> {
    Ok(Contract(ua.clone())
        .call_function("get_target_state_version", json!({}))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

pub async fn needs_migration(network: &NetworkConfig, ua: &AccountId) -> Result<bool> {
    Ok(Contract(ua.clone())
        .call_function("needs_migration", json!({}))
        .read_only()
        .fetch_from(network)
        .await?
        .data)
}

/// A raw view that surfaces the RPC error (used to assert a view *breaks* when
/// the stored state version is corrupted).
pub async fn view_succeeds(network: &NetworkConfig, account_id: &AccountId, method: &str) -> bool {
    Contract(account_id.clone())
        .call_function(method, json!({}))
        .read_only::<serde_json::Value>()
        .fetch_from(network)
        .await
        .is_ok()
}

/// Patch contract storage entries (raw key/value byte pairs) on `account_id`.
pub async fn patch_storage(
    harness: &SandboxHarness,
    account_id: &AccountId,
    entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    harness.patch_state(account_id, entries).await
}

/// Patch the stored state-version key (`__v`) with raw bytes.
pub async fn patch_state_version(
    harness: &SandboxHarness,
    account_id: &AccountId,
    bytes: &[u8],
) -> Result<()> {
    patch_storage(harness, account_id, [(b"__v".to_vec(), bytes.to_vec())]).await
}

/// Build a `mint` function-call action for a universal-account transaction.
pub fn mint_action(amount: u128) -> FunctionCallAction {
    FunctionCallAction {
        function_name: "mint".to_string(),
        arguments: serde_json::to_vec(&json!({ "amount": U128(amount) }))
            .expect("serialize mint args")
            .into(),
        amount: NearToken::from_near(0),
        gas: Gas::from_tgas(30),
    }
}

/// Build an `increment` function-call action for a universal-account transaction.
pub fn increment_action() -> FunctionCallAction {
    FunctionCallAction {
        function_name: "increment".to_string(),
        arguments: json!({}).to_string().into_bytes().into(),
        amount: NearToken::from_near(0),
        gas: Gas::from_tgas(30),
    }
}
