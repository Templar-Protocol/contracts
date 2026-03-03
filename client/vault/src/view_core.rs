use std::{sync::RwLock, time::Duration};

use anyhow::{anyhow, bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_jsonrpc_client::{methods::query::RpcQueryRequest, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{types::BlockReference, views::QueryRequest};
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

use crate::{lock_ext::RwLockExt, retry, KeyPoolConfig, ViewCache, ViewCacheKey};

/// Execute a view call with optional caching and retry logic.
///
/// This is the shared implementation used by both `KeyPoolClient` and `VaultViewClient`.
#[instrument(skip(inner, config, cache, args), fields(account_id = %account_id, method = function_name))]
pub(crate) async fn view_with_cache<T: DeserializeOwned>(
    inner: &JsonRpcClient,
    config: &KeyPoolConfig,
    cache: &RwLock<Option<ViewCache>>,
    account_id: &NearAccountId,
    function_name: &str,
    args: impl Serialize,
) -> Result<T> {
    let args_bytes = serde_json::to_vec(&args)?;
    let key = ViewCacheKey {
        account_id: account_id.to_string(),
        method: function_name.to_string(),
        args: args_bytes.clone(),
    };

    let cache_snapshot = {
        cache
            .read_or_poison()
            .map_err(|e| anyhow!("view cache lock failed: {e}"))?
            .clone()
    };
    if let Some(ref c) = cache_snapshot {
        if let Some(bytes) = c.get(&key) {
            let value = serde_json::from_slice(&bytes)?;
            return Ok(value);
        }
    }

    let timeout = Duration::from_secs(config.timeout_seconds);
    let mut retry_state = retry::RetryState::new(config.retry);

    loop {
        retry_state.begin_attempt();

        let response = tokio::time::timeout(
            timeout,
            inner.call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: account_id.clone(),
                    method_name: function_name.to_owned(),
                    args: args_bytes.clone().into(),
                },
            }),
        )
        .await;

        let response = match response {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let err: anyhow::Error = e.into();
                if !retry_state.should_retry_err(&err).await {
                    return Err(err);
                }
                continue;
            }
            Err(e) => {
                let err: anyhow::Error = e.into();
                if !retry_state.should_retry_err(&err).await {
                    return Err(err);
                }
                continue;
            }
        };

        let QueryResponseKind::CallResult(result) = response.kind else {
            bail!("Expected CallResult got {:?}", response.kind);
        };

        if let Some(ref c) = cache_snapshot {
            c.insert(key.clone(), result.result.clone());
        }

        let value = serde_json::from_slice(&result.result)?;
        return Ok(value);
    }
}

/// Build a view cache from configuration.
///
/// Returns `None` if the cache capacity is 0 (disabled).
pub(crate) fn build_view_cache(config: &KeyPoolConfig) -> Option<ViewCache> {
    if config.view_cache_capacity > 0 {
        Some(
            ViewCache::builder()
                .max_capacity(u64::from(config.view_cache_capacity))
                .time_to_live(Duration::from_secs(config.view_cache_ttl_seconds))
                .build(),
        )
    } else {
        None
    }
}
