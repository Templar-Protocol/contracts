//! Shared helpers for the gateway-`SandboxHarness`-based relayer integration
//! tests. Reads are typed `near_api` view calls; writes are signed `near_api`
//! transactions. Every sandbox account the harness provisions shares the same
//! well-known test key, so a single [`signer`] signs for any of them (the relay
//! account, the UA registry, mock oracles, ad-hoc users, ...).
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use near_api::{Contract, NetworkConfig, SecretKey, Signer};
use near_sdk::serde::{de::DeserializeOwned, Serialize};
use near_sdk::Gas;
use near_token::NearToken;
use serde_json::json;
use templar_gateway_testing::SandboxHarness;
use tokio::sync::OnceCell;

/// A process-wide reqwest client, built once so tx-status checks reuse one
/// connection pool instead of standing up a fresh client per call.
async fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceCell<reqwest::Client> = OnceCell::const_new();
    CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("failed to build reqwest client")
        })
        .await
}

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
    harness: &SandboxHarness,
    label: &str,
) -> Result<near_api::types::AccountId> {
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
    deposit: NearToken,
) -> Result<()> {
    Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .gas(Gas::from_tgas(gas_tgas))
        .deposit(deposit)
        .with_signer(signer_id.clone(), signer()?)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Deploy raw wasm to `account_id` with no init call.
pub async fn deploy_code(
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

/// Stand up a fresh registry contract under a new account and initialize it
/// (owner = the registry account). The harness `deploy_registry` targets a
/// single fixed account, so the relayer's second (UA) registry is deployed here.
pub async fn deploy_registry(
    harness: &SandboxHarness,
    label: &str,
) -> Result<near_api::types::AccountId> {
    let registry_id = create_account(harness, label).await?;
    deploy_code(
        &harness.network,
        &registry_id,
        templar_gateway_testing::wasm::registry().await.to_vec(),
    )
    .await?;
    call(
        &harness.network,
        &registry_id,
        &registry_id,
        "new",
        json!({}),
        50,
        NearToken::from_yoctonear(0),
    )
    .await?;
    Ok(registry_id)
}

/// Assert a relayed transaction reached a successful final execution outcome,
/// via the JSON-RPC `tx` query (the near-workspaces `worker.tx_status(...)
/// .assert_success()` replacement).
pub async fn assert_tx_succeeded(
    network: &NetworkConfig,
    tx_hash: near_primitives::hash::CryptoHash,
    sender_id: &near_api::types::AccountId,
) -> Result<()> {
    let url = network.rpc_endpoints[0].url.clone();
    let response: serde_json::Value = http_client()
        .await
        .post(url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "tx",
            "method": "tx",
            "params": {
                "tx_hash": tx_hash.to_string(),
                "sender_account_id": sender_id.to_string(),
                "wait_until": "FINAL",
            },
        }))
        .send()
        .await?
        .json()
        .await?;
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        anyhow::bail!("tx status error for {tx_hash}: {error}");
    }
    let status = &response["result"]["status"];
    anyhow::ensure!(
        status.get("SuccessValue").is_some() || status.get("SuccessReceiptId").is_some(),
        "transaction {tx_hash} did not succeed: {status}"
    );
    Ok(())
}
