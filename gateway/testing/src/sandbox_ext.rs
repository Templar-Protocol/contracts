//! Break-glass direct JSON-RPC to the sandbox node, deliberately bypassing the
//! gateway.
//!
//! The harness mints test accounts by writing state records with
//! `sandbox_patch_state` — far cheaper than a real `CreateAccount` transaction —
//! advances chain time with `sandbox_fast_forward`, and reads access keys at a
//! chosen finality. Those are sandbox-only RPC extensions (or finality controls)
//! the gateway neither has nor should ever wrap, so all raw-RPC access is confined
//! to this module; the rest of the harness goes through the gateway.
//! [`near_jsonrpc_client`] supplies the typed request/response plumbing.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use near_api::{types::AccountId, NetworkConfig, SecretKey};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest, sandbox_fast_forward::RpcSandboxFastForwardRequest,
        sandbox_patch_state::RpcSandboxPatchStateRequest,
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryError};
use near_primitives::{
    account::{AccessKey, Account as ChainAccount, AccountContract},
    state_record::StateRecord,
    types::{BlockReference, Finality},
    views::{AccessKeyPermissionView, QueryRequest},
};
use near_token::NearToken;

/// A JSON-RPC client for `network`.
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
    let public_key = public_key(secret_key)?;
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
            public_key: public_key.clone(),
            access_key: AccessKey::full_access(),
        },
    ];
    patch_records(network, records).await?;
    wait_until_final(network, account_id, &public_key).await
}

/// Write `records` directly into chain state via `sandbox_patch_state`.
pub(crate) async fn patch_records(
    network: &NetworkConfig,
    records: Vec<StateRecord>,
) -> Result<()> {
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

/// List `account_id`'s access keys as `(public_key, is_full_access)` at final
/// finality.
pub(crate) async fn view_access_keys(
    network: &NetworkConfig,
    account_id: &AccountId,
) -> Result<Vec<(String, bool)>> {
    let response = client(network)
        .call(RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::Final),
            request: QueryRequest::ViewAccessKeyList {
                account_id: account_id.clone(),
            },
        })
        .await
        .context("view_access_key_list failed")?;
    let QueryResponseKind::AccessKeyList(list) = response.kind else {
        bail!("unexpected query response kind for view_access_key_list");
    };
    Ok(list
        .keys
        .into_iter()
        .map(|key| {
            let full_access = matches!(
                key.access_key.permission,
                AccessKeyPermissionView::FullAccess
            );
            (key.public_key.to_string(), full_access)
        })
        .collect())
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
    let mut last_error = None;
    for _ in 0..200 {
        match client.call(&request).await {
            // The key is present at final finality.
            Ok(_) => return Ok(()),
            Err(error) => match error.handler_error() {
                // The account or key merely hasn't reached final finality yet.
                Some(
                    RpcQueryError::UnknownAccessKey { .. } | RpcQueryError::UnknownAccount { .. },
                ) => {}
                // A genuine query error (malformed request, node error): fail fast
                // rather than spinning to the timeout.
                Some(other) => bail!("view_access_key for {account_id} failed: {other}"),
                // A transport blip: transient, keep polling.
                None => last_error = Some(error.to_string()),
            },
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    bail!(
        "patched account {account_id} did not reach final finality in time{}",
        last_error.map_or(String::new(), |error| format!(" (last error: {error})"))
    )
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
