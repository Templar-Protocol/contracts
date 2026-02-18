//! Nonce fetching utilities for NEAR access keys.

use std::time::Duration;

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_crypto::PublicKey;
use near_jsonrpc_client::{methods::query::RpcQueryRequest, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{hash::CryptoHash, types::BlockReference, views::QueryRequest};

/// Fetch the current nonce and recent block hash for an access key.
///
/// Returns `(next_nonce, block_hash)` where `next_nonce` is already incremented
/// (i.e., the nonce to use for the next transaction).
pub async fn fetch_access_key_data(
    rpc: &JsonRpcClient,
    account_id: NearAccountId,
    public_key: PublicKey,
    timeout: Duration,
) -> Result<(u64, CryptoHash)> {
    let response = tokio::time::timeout(
        timeout,
        rpc.call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id,
                public_key,
            },
        }),
    )
    .await??;

    let nonce = match response.kind {
        QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
        other => {
            bail!("Expected AccessKey response, got {other:?}");
        }
    };

    Ok((nonce, response.block_hash))
}

#[cfg(test)]
mod tests {
    // Integration tests would require a live RPC endpoint
}
