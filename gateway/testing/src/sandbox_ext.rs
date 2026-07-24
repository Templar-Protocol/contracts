//! Sandbox-only JSON-RPC that neither the gateway nor near-api wraps:
//! `sandbox_patch_state` (used to mint test accounts far more cheaply than a real
//! `CreateAccount` transaction) and `sandbox_fast_forward`. All raw-RPC access is
//! confined to this module.

use std::time::Duration;

use anyhow::{Context, Result};
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

/// Storage bytes for an account holding one ed25519 full-access key:
/// `num_bytes_account` 100 + borsh public key 33 + borsh `AccessKey` 9 +
/// `num_extra_bytes_record` 40.
const ACCOUNT_STORAGE_USAGE: u64 = 182;

const RPC_TIMEOUT: Duration = Duration::from_secs(120);

/// The custom client is what carries [`RPC_TIMEOUT`] — `connect`'s default sets
/// none — and so must also set the content-type header it would have: the node
/// answers 415 without it.
fn client(network: &NetworkConfig) -> JsonRpcClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    let http = reqwest::Client::builder()
        .timeout(RPC_TIMEOUT)
        .default_headers(headers)
        .build()
        .expect("reqwest client builds");
    JsonRpcClient::with(http).connect(network.rpc_endpoints[0].url.as_str())
}

/// Mint each account with its balance and a full-access key over `secret_key` by
/// writing state records directly — no blocks spent producing them, and no
/// funder is debited.
///
/// One call, however many accounts: a `sandbox_patch_state` call costs ~200ms
/// regardless of how many records it carries, so minting N accounts one call at
/// a time costs N times as much as minting them together.
pub(crate) async fn create_accounts(
    network: &NetworkConfig,
    accounts: &[(AccountId, NearToken)],
    secret_key: &SecretKey,
) -> Result<()> {
    // near-api and `near_primitives` use different `PublicKey` types; cross the
    // boundary by the canonical `ed25519:…` string.
    let public_key: near_crypto::PublicKey = secret_key
        .public_key()
        .to_string()
        .parse()
        .context("parse public key")?;
    // Nonce 0 is safe: the block-height nonce floor applies only to keys added by a
    // transaction, not to patched state.
    let records = accounts
        .iter()
        .flat_map(|(account_id, balance)| {
            [
                StateRecord::Account {
                    account_id: account_id.clone(),
                    account: ChainAccount::new(
                        *balance,
                        NearToken::from_yoctonear(0),
                        AccountContract::None,
                        ACCOUNT_STORAGE_USAGE,
                    ),
                },
                StateRecord::AccessKey {
                    account_id: account_id.clone(),
                    public_key: public_key.clone(),
                    access_key: AccessKey::full_access(),
                },
            ]
        })
        .collect();
    patch_records(network, records).await?;
    // One patch applies as a unit in one block, so any one account reaching
    // final finality means every account in the batch has.
    match accounts.last() {
        Some((account_id, _)) => wait_until_final(network, account_id, &public_key).await,
        None => Ok(()),
    }
}

/// Patch raw contract storage entries (key/value byte pairs) on `account_id`.
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

async fn patch_records(network: &NetworkConfig, records: Vec<StateRecord>) -> Result<()> {
    client(network)
        .call(RpcSandboxPatchStateRequest { records })
        .await
        .context("sandbox_patch_state failed")?;
    Ok(())
}

/// Advance the sandbox chain by `delta_height` blocks.
pub(crate) async fn fast_forward(network: &NetworkConfig, delta_height: u64) -> Result<()> {
    client(network)
        .call(RpcSandboxFastForwardRequest { delta_height })
        .await
        .context("sandbox_fast_forward failed")?;
    Ok(())
}

/// How long a patched key gets to reach `Final`.
///
/// Finality is a couple of blocks on an idle node, but this has to hold on a
/// CPU-starved CI runner where several nodes compete for a core and block
/// production stretches far past its target. Erring long costs nothing on a
/// healthy node — the wait ends as soon as the key is visible — while erring
/// short turns load into a spurious, confusing test failure.
const FINALITY_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll backoff bounds. Starts tight so an idle node is not slowed down, then
/// backs off: a flat fast poll from every test process piles RPC load onto the
/// very node that is already struggling.
const FINALITY_POLL_MIN: Duration = Duration::from_millis(25);
const FINALITY_POLL_MAX: Duration = Duration::from_millis(500);

/// Block until the patched key is visible at `Final`.
///
/// A patch lands at optimistic finality, but near-api's signer reads the nonce at
/// a hardcoded `Final`, which lags ~2 blocks — so without this the account's first
/// transaction fails with "access key ... does not exist".
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
    // Every error retries: not-yet-final, node backpressure and transport blips all
    // clear on a later block.
    tokio::time::timeout(FINALITY_TIMEOUT, async {
        let mut backoff = FINALITY_POLL_MIN;
        while client.call(&request).await.is_err() {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(FINALITY_POLL_MAX);
        }
    })
    .await
    .with_context(|| {
        format!(
            "patched account {account_id} never reached final finality within \
             {FINALITY_TIMEOUT:?} — the sandbox node is likely overloaded or down"
        )
    })
}
