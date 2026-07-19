//! Break-glass direct JSON-RPC to the sandbox node, deliberately bypassing the
//! gateway.
//!
//! The harness mints test accounts by writing state records with
//! `sandbox_patch_state` — far cheaper than a real `CreateAccount` transaction —
//! advances chain time with `sandbox_fast_forward`, and polls an access key at
//! `Final` while a patch settles. Those are sandbox-only RPC extensions (or
//! finality reads near-api can't express) the gateway neither has nor should ever
//! wrap, so all raw-RPC access is confined to this module; the rest of the
//! harness goes through the gateway or near-api. [`near_jsonrpc_client`] supplies
//! the typed request/response plumbing.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use near_api::{types::AccountId, NetworkConfig, SecretKey};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest, sandbox_fast_forward::RpcSandboxFastForwardRequest,
        sandbox_patch_state::RpcSandboxPatchStateRequest,
    },
    JsonRpcClient,
};
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    state_record::StateRecord,
    types::{BlockReference, Finality, StoreKey, StoreValue},
    views::QueryRequest,
};
use near_token::NearToken;

fn client(network: &NetworkConfig) -> JsonRpcClient {
    JsonRpcClient::connect(network.rpc_endpoints[0].url.as_str())
}

/// Mint `account_id` with `balance` and a full-access key over `secret_key`,
/// written directly into chain state via `sandbox_patch_state`. Unlike a real
/// `CreateAccount` transaction this costs zero blocks and debits no funder (the
/// balance is minted), which is what makes it far cheaper.
pub(crate) async fn create_account(
    network: &NetworkConfig,
    account_id: &AccountId,
    secret_key: &SecretKey,
    balance: NearToken,
) -> Result<()> {
    let pk = public_key(secret_key)?;
    // `AccountContract::None` is a codeless account (all-zero code hash); `182` is
    // near's canonical storage size for one full-access key. The key nonce starts
    // at 0 — the block-height nonce floor applies only to keys added by a
    // transaction, not to patched state.
    let records = vec![
        StateRecord::Account {
            account_id: account_id.clone(),
            account: ChainAccount::new(
                balance,
                NearToken::from_yoctonear(0),
                AccountContract::None,
                182,
            ),
        },
        StateRecord::AccessKey {
            account_id: account_id.clone(),
            public_key: pk.clone(),
            access_key: AccessKey::full_access(),
        },
    ];
    patch_records(network, records).await?;
    wait_until_final(network, account_id, &pk).await
}

/// Patch raw contract storage entries (key/value byte pairs) on `account_id`
/// via `sandbox_patch_state`.
pub(crate) async fn patch_data(
    network: &NetworkConfig,
    account_id: &AccountId,
    entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    let records = entries
        .into_iter()
        .map(|(key, value)| StateRecord::Data {
            account_id: account_id.clone(),
            data_key: StoreKey::from(key),
            value: StoreValue::from(value),
        })
        .collect();
    patch_records(network, records).await
}

/// Write `records` directly into chain state via `sandbox_patch_state`.
async fn patch_records(network: &NetworkConfig, records: Vec<StateRecord>) -> Result<()> {
    client(network)
        .call(RpcSandboxPatchStateRequest { records })
        .await
        .context("sandbox_patch_state failed")?;
    Ok(())
}

/// Advance the sandbox chain by `delta_height` blocks via `sandbox_fast_forward`.
pub(crate) async fn fast_forward(network: &NetworkConfig, delta_height: u64) -> Result<()> {
    client(network)
        .call(RpcSandboxFastForwardRequest { delta_height })
        .await
        .context("sandbox_fast_forward failed")?;
    Ok(())
}

/// Block until `public_key`'s access key on `account_id` is visible at
/// `Finality::Final`.
///
/// `sandbox_patch_state` takes effect immediately at `optimistic` finality, but
/// near-api hardcodes `Finality::Final` for every query it issues — including the
/// nonce/access-key read on the first transaction the account signs — and `Final`
/// lags the head by ~2 blocks. Without this wait that first transaction races the
/// patch and fails with "access key ... does not exist".
async fn wait_until_final(
    network: &NetworkConfig,
    account_id: &AccountId,
    public_key: &near_crypto::PublicKey,
) -> Result<()> {
    let client = client(network);
    let request = RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::ViewAccessKey {
            account_id: account_id.clone(),
            public_key: public_key.clone(),
        },
    };
    // Every error is retried until the deadline: the key not-yet-final
    // (`UnknownAccessKey`/`UnknownAccount`), node backpressure ("reached its
    // limits"), and transport blips all resolve on a later block, so a
    // transiently-busy pooled node must not fail account creation.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match client.call(&request).await {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => bail!(
                "patched account {account_id} did not reach final finality \
                 within 10s (last error: {error})"
            ),
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The chain-typed (`near_crypto`) public key for a near-api secret key. near-api
/// and `near_primitives` use different `PublicKey` types, so cross the boundary by
/// the canonical `ed25519:…` string.
fn public_key(secret_key: &SecretKey) -> Result<near_crypto::PublicKey> {
    secret_key
        .public_key()
        .to_string()
        .parse()
        .context("parse public key")
}
