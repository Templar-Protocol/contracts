use std::{
    str::FromStr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::{
    methods::{
        query::RpcQueryRequest,
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{BlockReference, Gas},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, instrument, warn};
use zeroize::Zeroize;

use crate::{
    lock_ext::RwLockExt, parse_account_id, AccountId, AllocationDelta, CapGroup, CapGroupUpdate,
    CapGroupUpdateKey, ErrorWrapper, FeeAccrualAnchor, Fees, ForeignU128, MarketId, MarketWithId,
    PendingGovernanceAction, PendingValueSerde, RealAssetsReport, Restrictions, RetryConfig,
    TimelockKind, VaultConfiguration, VaultSnapshot, ViewCache, ViewCacheKey, DEFAULT_GAS,
    MAX_POLL_INTERVAL_MILLIS,
};

use super::{health::PoolHealth, pool::KeyPool, slot::KeySlot};

/// Credentials for a single NEAR access key.
#[derive(uniffi::Record, Clone)]
pub struct KeyCredential {
    /// The account ID that owns this access key.
    pub account_id: AccountId,
    /// The secret key in string form (e.g., "ed25519:...").
    pub secret_key: String,
}

impl KeyCredential {
    fn into_signer(mut self) -> Result<InMemorySigner, ErrorWrapper> {
        // Helper to ensure secret is always zeroed, even on error paths
        let zeroize_secret = |s: &mut String| {
            // SAFETY: as_bytes_mut on String is safe; we're just zeroing the bytes
            unsafe { s.as_bytes_mut().zeroize() };
        };

        let account_id = match parse_account_id(&self.account_id) {
            Ok(id) => id,
            Err(e) => {
                zeroize_secret(&mut self.secret_key);
                return Err(e);
            }
        };

        let secret_key = match SecretKey::from_str(&self.secret_key) {
            Ok(k) => k,
            Err(e) => {
                zeroize_secret(&mut self.secret_key);
                return Err(ErrorWrapper::Wrapped(e.to_string()));
            }
        };

        // Zero the source string now that we've parsed it
        zeroize_secret(&mut self.secret_key);

        Ok(InMemorySigner {
            account_id,
            public_key: secret_key.public_key(),
            secret_key,
        })
    }
}

/// Configuration for KeyPoolClient.
#[derive(uniffi::Record, Clone)]
pub struct KeyPoolConfig {
    /// Default timeout for RPC calls in seconds.
    pub timeout_seconds: u64,

    /// Retry configuration for transient errors.
    pub retry: Option<RetryConfig>,

    /// Maximum nonce retry attempts specifically for InvalidNonce errors.
    pub max_nonce_retries: u32,

    /// Block hash TTL in seconds (for key slot nonce caching).
    pub block_hash_ttl_seconds: u64,

    /// View cache capacity (0 = disabled).
    pub view_cache_capacity: u32,

    /// View cache TTL in seconds.
    pub view_cache_ttl_seconds: u64,
}

impl Default for KeyPoolConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 60,
            retry: Some(RetryConfig {
                max_attempts: 3,
                initial_backoff_ms: 100,
                max_backoff_ms: 5000,
            }),
            max_nonce_retries: 3,
            block_hash_ttl_seconds: 30,
            view_cache_capacity: 100,
            view_cache_ttl_seconds: 5,
        }
    }
}

/// Pool-aware vault client with automatic key selection and nonce management.
///
/// This is a drop-in replacement for `Client` that supports multiple access keys
/// for concurrent transaction submission.
#[derive(uniffi::Object)]
pub struct KeyPoolClient {
    /// JSON-RPC client connection.
    inner: JsonRpcClient,

    /// The vault contract account.
    vault: NearAccountId,

    /// The key pool for transaction signing.
    pool: KeyPool,

    /// Configuration.
    config: KeyPoolConfig,

    /// View cache (optional).
    view_cache: RwLock<Option<ViewCache>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl KeyPoolClient {
    /// Create a new KeyPoolClient.
    ///
    /// # Arguments
    /// * `rpc_url` - NEAR RPC endpoint URL
    /// * `vault` - Vault contract account ID
    /// * `credentials` - List of key credentials (at least one required)
    /// * `config` - Client configuration
    ///
    /// # Errors
    /// Returns error if credentials is empty or any credential is invalid.
    #[uniffi::constructor]
    #[instrument(skip(credentials, config), fields(rpc_url = %rpc_url))]
    pub fn new(
        rpc_url: String,
        vault: &AccountId,
        credentials: Vec<KeyCredential>,
        config: KeyPoolConfig,
    ) -> Result<Self, ErrorWrapper> {
        if credentials.is_empty() {
            return Err(ErrorWrapper::Wrapped(
                "credentials cannot be empty".to_string(),
            ));
        }

        let inner = JsonRpcClient::connect(rpc_url);
        let vault: NearAccountId = parse_account_id(vault)?;

        let block_hash_ttl = Duration::from_secs(config.block_hash_ttl_seconds);
        let slots: Vec<Arc<KeySlot>> = credentials
            .into_iter()
            .map(|c| {
                let signer = c.into_signer()?;
                Ok(Arc::new(KeySlot::with_config(signer, block_hash_ttl)))
            })
            .collect::<Result<Vec<_>, ErrorWrapper>>()?;

        let pool = KeyPool::from_slots(slots).map_err(|e| ErrorWrapper::Wrapped(e.to_string()))?;

        let view_cache = if config.view_cache_capacity > 0 {
            Some(
                ViewCache::builder()
                    .max_capacity(config.view_cache_capacity as u64)
                    .time_to_live(Duration::from_secs(config.view_cache_ttl_seconds))
                    .build(),
            )
        } else {
            None
        };

        Ok(Self {
            inner,
            vault,
            pool,
            config,
            view_cache: RwLock::new(view_cache),
        })
    }

    /// Get health status of the key pool.
    pub fn get_pool_health(&self) -> PoolHealth {
        PoolHealth::from_pool(&self.pool)
    }

    /// Get the vault account ID.
    pub fn vault_account(&self) -> AccountId {
        AccountId::from(self.vault.to_string())
    }

    pub fn enable_view_cache(&self, capacity: u32, ttl_seconds: u64) {
        if capacity == 0 {
            *self.view_cache.write_recover() = None;
            return;
        }

        let cache = ViewCache::builder()
            .max_capacity(capacity as u64)
            .time_to_live(Duration::from_secs(ttl_seconds))
            .build();

        *self.view_cache.write_recover() = Some(cache);
    }

    pub fn disable_view_cache(&self) {
        *self.view_cache.write_recover() = None;
    }

    pub async fn clear_view_cache(&self) -> Result<(), ErrorWrapper> {
        let cache = { self.view_cache.read_recover().clone() };
        if let Some(cache) = cache {
            cache.invalidate_all();
        }
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn get_cap_groups(&self) -> Result<Vec<CapGroup>, ErrorWrapper> {
        let groups = self
            .view::<Vec<(
                templar_common::vault::CapGroupId,
                templar_common::vault::CapGroupRecord,
            )>>(&self.vault, "get_cap_groups", ())
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(groups
            .into_iter()
            .map(|(id, rec)| CapGroup {
                id: id.into(),
                cap: rec.cap.0.to_string(),
                relative_cap: u128::from(rec.relative_cap).to_string(),
                principal: rec.principal.to_string(),
            })
            .collect())
    }

    #[instrument(skip(self))]
    pub async fn get_pending_governance_actions(
        &self,
    ) -> Result<Vec<PendingGovernanceAction>, ErrorWrapper> {
        let pending = self
            .view::<Vec<PendingValueSerde>>(&self.vault, "get_pending_governance_actions", ())
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(pending
            .into_iter()
            .map(|p| PendingGovernanceAction {
                action: p.value.into(),
                valid_at_ns: p.valid_at_ns,
            })
            .collect())
    }

    #[instrument(skip(self, market))]
    pub async fn get_market_id_of_account(
        &self,
        market: &AccountId,
    ) -> Result<Option<MarketId>, ErrorWrapper> {
        let res = self
            .view::<Option<U64>>(
                &self.vault,
                "get_market_id_of_account",
                (self.near_id(market)?,),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        let Some(u) = res else {
            return Ok(None);
        };

        let id_u32: u32 =
            u.0.try_into()
                .map_err(|_| ErrorWrapper::Wrapped("market id out of u32 range".to_string()))?;

        Ok(Some(MarketId(id_u32)))
    }

    #[instrument(skip(self, market_id))]
    pub async fn get_market_account_by_id(
        &self,
        market_id: MarketId,
    ) -> Result<Option<AccountId>, ErrorWrapper> {
        let res = self
            .view::<Option<NearAccountId>>(
                &self.vault,
                "get_market_account_by_id",
                (U64::from(market_id.0 as u64),),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(res.map(|a| AccountId::from(a.to_string())))
    }

    #[instrument(skip(self))]
    pub async fn list_markets_with_ids(&self) -> Result<Vec<MarketWithId>, ErrorWrapper> {
        let res = self
            .view::<Vec<(U64, NearAccountId)>>(&self.vault, "list_markets_with_ids", ())
            .await
            .map_err(ErrorWrapper::from)?;

        let mapped =
            res.into_iter()
                .map(|(id, account)| {
                    let id_u32: u32 = id.0.try_into().map_err(|_| {
                        ErrorWrapper::Wrapped("market id out of u32 range".to_string())
                    })?;
                    Ok(MarketWithId {
                        market_id: MarketId(id_u32),
                        account: AccountId::from(account.to_string()),
                    })
                })
                .collect::<Result<Vec<_>, ErrorWrapper>>()?;

        Ok(mapped)
    }

    #[instrument(skip(self))]
    pub async fn get_vault_snapshot(&self) -> Result<VaultSnapshot, ErrorWrapper> {
        Ok(VaultSnapshot {
            configuration: self.get_configuration().await?,
            total_assets: self.get_total_assets().await?,
            last_total_assets: self.get_last_total_assets().await?,
            idle_balance: self.get_idle_balance().await?,
            total_supply: self.get_total_supply().await?,
            max_deposit: self.get_max_deposit().await?,
            max_single_market_deposit: self.get_max_single_market_deposit().await?,
            fee_anchor: self.get_fee_anchor().await?,
            fees: self.get_fees().await?,
            restrictions: self.get_restrictions().await?,
            cap_groups: self.get_cap_groups().await?,
            pending_governance_actions: self.get_pending_governance_actions().await?,
            withdrawing_op_id: self.get_withdrawing_op_id().await?,
            has_pending_market_withdrawal: self.has_pending_market_withdrawal().await?,
            current_withdraw_request_id: self.get_current_withdraw_request_id().await?,
            queue_tail: self.queue_tail().await?,
            next_pending_withdrawal_id: self.peek_next_pending_withdrawal_id().await?,
            markets_with_ids: self.list_markets_with_ids().await?,
        })
    }

    #[instrument(skip(self, markets))]
    pub async fn resolve_market_ids(
        &self,
        markets: &[AccountId],
    ) -> Result<Vec<Option<MarketId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(markets.len());
        for market in markets {
            out.push(self.get_market_id_of_account(market).await?);
        }
        Ok(out)
    }

    #[instrument(skip(self, market_ids))]
    pub async fn resolve_market_accounts(
        &self,
        market_ids: &[MarketId],
    ) -> Result<Vec<Option<AccountId>>, ErrorWrapper> {
        let mut out = Vec::with_capacity(market_ids.len());
        for id in market_ids {
            out.push(self.get_market_account_by_id(*id).await?);
        }
        Ok(out)
    }

    #[instrument(skip(self))]
    pub async fn refresh_all_markets(&self) -> Result<RealAssetsReport, ErrorWrapper> {
        let markets = self.list_markets_with_ids().await?;
        let market_ids: Vec<MarketId> = markets.into_iter().map(|m| m.market_id).collect();
        self.refresh_markets(&market_ids).await
    }
}

impl KeyPoolClient {
    /// Execute a view call with optional caching.
    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name))]
    pub(crate) async fn view<T: DeserializeOwned>(
        &self,
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

        let cache = { self.view_cache.read_recover().clone() };
        if let Some(cache) = &cache {
            if let Some(bytes) = cache.get(&key) {
                let value = serde_json::from_slice(&bytes)?;
                return Ok(value);
            }
        }

        let timeout = Duration::from_secs(self.config.timeout_seconds);
        let retry = self.config.retry.map(|r| r.normalized());
        let mut attempts_left = retry.map(|r| r.max_attempts).unwrap_or(1);
        let mut backoff_ms = retry.map(|r| r.initial_backoff_ms).unwrap_or(0);

        loop {
            attempts_left = attempts_left.saturating_sub(1);

            let response = tokio::time::timeout(
                timeout,
                self.inner.call(RpcQueryRequest {
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
                    if attempts_left == 0 || !should_retry(&err) {
                        return Err(err);
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    if let Some(r) = retry {
                        backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                    }
                    continue;
                }
                Err(e) => {
                    let err: anyhow::Error = e.into();
                    if attempts_left == 0 || !should_retry(&err) {
                        return Err(err);
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    if let Some(r) = retry {
                        backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                    }
                    continue;
                }
            };

            let QueryResponseKind::CallResult(result) = response.kind else {
                bail!("Expected CallResult got {:?}", response.kind);
            };

            if let Some(cache) = &cache {
                cache.insert(key.clone(), result.result.clone());
            }

            let value = serde_json::from_slice(&result.result)?;
            return Ok(value);
        }
    }

    /// Execute a state-changing contract call with pool-aware nonce management.
    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name, gas = ?gas, deposit = ?deposit))]
    pub(crate) async fn call(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
    ) -> Result<FinalExecutionStatus> {
        let args_bytes = serde_json::to_vec(&args)?;
        let timeout = Duration::from_secs(self.config.timeout_seconds);

        let mut nonce_retries = self.config.max_nonce_retries;

        loop {
            let slot = self.pool.select().map_err(|e| anyhow::anyhow!("{}", e))?;
            let guard = slot.acquire().await;

            let (nonce, block_hash) = match guard.next_nonce(&self.inner, timeout).await {
                Ok(data) => data,
                Err(e) => {
                    guard.record_failure();
                    return Err(e);
                }
            };

            let tx = Transaction::V0(TransactionV0 {
                nonce,
                receiver_id: account_id.clone(),
                block_hash,
                signer_id: guard.signer().account_id.clone(),
                public_key: guard.signer().public_key().clone(),
                actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: function_name.to_string(),
                    args: args_bytes.clone(),
                    gas: gas.unwrap_or(DEFAULT_GAS),
                    deposit: deposit.unwrap_or(0),
                }))],
            });

            let (tx_hash, _size) = tx.get_hash_and_size();
            let sender_account_id = guard.signer().account_id.clone();
            let signature = guard.signer().sign(tx_hash.as_ref());
            let signed_transaction = SignedTransaction::new(signature, tx);

            let result = self
                .submit_and_poll(signed_transaction, sender_account_id, tx_hash, timeout)
                .await;

            match result {
                Ok(status) => {
                    guard.advance_nonce().await;

                    if let FinalExecutionStatus::Failure(tx_err) = &status {
                        guard.record_failure();
                        bail!("Transaction failed: {:?}", tx_err);
                    }

                    guard.record_success();
                    return Ok(status);
                }
                Err(e) => {
                    if Self::is_invalid_nonce_error(&e) && nonce_retries > 0 {
                        warn!(
                            "InvalidNonce error for key {}, retrying ({} attempts left)",
                            guard.signer().account_id,
                            nonce_retries
                        );
                        guard.invalidate_nonce().await;
                        nonce_retries -= 1;
                        continue;
                    }
                    guard.record_failure();
                    return Err(e);
                }
            }
        }
    }

    /// Submit transaction and poll for result.
    async fn submit_and_poll(
        &self,
        signed_transaction: SignedTransaction,
        sender_account_id: NearAccountId,
        tx_hash: CryptoHash,
        timeout: Duration,
    ) -> Result<FinalExecutionStatus> {
        let retry = self.config.retry.map(|r| r.normalized());
        let mut attempts_left = retry.map(|r| r.max_attempts).unwrap_or(1);
        let mut backoff_ms = retry.map(|r| r.initial_backoff_ms).unwrap_or(0);

        let deadline = Instant::now() + timeout;

        let result = loop {
            attempts_left = attempts_left.saturating_sub(1);

            let send_res = self
                .inner
                .call(RpcSendTransactionRequest {
                    signed_transaction: signed_transaction.clone(),
                    wait_until: TxExecutionStatus::ExecutedOptimistic,
                })
                .await;

            match send_res {
                Ok(res) => break Ok(res),
                Err(e) => {
                    if Self::is_rpc_invalid_nonce(&e) {
                        break Err(anyhow::anyhow!("InvalidNonce"));
                    }

                    if matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                        break Err(e.into());
                    }

                    if retry.is_none() || attempts_left == 0 || e.handler_error().is_some() {
                        break Err(e.into());
                    }

                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    if let Some(r) = retry {
                        backoff_ms = (backoff_ms.saturating_mul(2)).min(r.max_backoff_ms);
                    }
                }
            }
        };

        let result = match result {
            Ok(res) => res,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Timeout") || err_str.contains("timeout") {
                    warn!(
                        "Send transaction timeout: {:?}. Starting status polling until deadline.",
                        e
                    );
                    return self
                        .poll_tx_status(sender_account_id, tx_hash, deadline)
                        .await;
                }
                return Err(e);
            }
        };

        let Some(outcome) = result.final_execution_outcome else {
            bail!("No outcome {}", tx_hash);
        };

        let status = outcome.into_outcome().status;
        Ok(status)
    }

    /// Poll transaction status until deadline.
    async fn poll_tx_status(
        &self,
        sender_account_id: NearAccountId,
        tx_hash: CryptoHash,
        deadline: Instant,
    ) -> Result<FinalExecutionStatus> {
        let retry = self.config.retry.map(|r| r.normalized());
        let mut poll_interval = Duration::from_millis(500);

        // signer_account_id must match the transaction signer (NEAR uses it for shard routing)

        let result = loop {
            if Instant::now() >= deadline {
                warn!("Transaction polling deadline exceeded, aborting");
                bail!("Transaction timed out");
            }

            tokio::time::sleep(poll_interval).await;
            debug!("Polling transaction status...");

            poll_interval = std::cmp::min(
                poll_interval * 2,
                Duration::from_millis(MAX_POLL_INTERVAL_MILLIS),
            );

            let status = self
                .inner
                .call(RpcTransactionStatusRequest {
                    transaction_info: TransactionInfo::TransactionId {
                        sender_account_id: sender_account_id.clone(),
                        tx_hash,
                    },
                    wait_until: TxExecutionStatus::ExecutedOptimistic,
                })
                .await;

            match status {
                Ok(res) => break res,
                Err(status_err) => {
                    if matches!(
                        status_err.handler_error(),
                        Some(RpcTransactionError::TimeoutError)
                    ) {
                        continue;
                    }

                    if retry.is_some() && status_err.handler_error().is_none() {
                        continue;
                    }

                    warn!("Transaction status error: {:?}", status_err);
                    return Err(status_err.into());
                }
            }
        };

        let Some(outcome) = result.final_execution_outcome else {
            bail!("No outcome {}", tx_hash);
        };

        let status = outcome.into_outcome().status;
        Ok(status)
    }

    /// Check if an error indicates an InvalidNonce condition.
    fn is_invalid_nonce_error(err: &anyhow::Error) -> bool {
        let err_str = err.to_string();
        err_str.contains("InvalidNonce") || err_str.contains("invalid nonce")
    }

    /// Check RPC error for InvalidNonce.
    fn is_rpc_invalid_nonce(
        err: &near_jsonrpc_client::errors::JsonRpcError<RpcTransactionError>,
    ) -> bool {
        if let Some(handler_err) = err.handler_error() {
            let err_str = format!("{:?}", handler_err);
            return err_str.contains("InvalidNonce") || err_str.contains("invalid nonce");
        }
        false
    }

    #[inline]
    fn near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper> {
        parse_account_id(id)
    }

    async fn vault_view_u128(
        &self,
        method: &str,
        args: impl Serialize,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let u = self
            .view::<U128>(&self.vault, method, args)
            .await
            .map_err(ErrorWrapper::from)?;
        Ok(u.0.to_string())
    }

    async fn vault_call_with(
        &self,
        method: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
    ) -> Result<(), ErrorWrapper> {
        self.call(&self.vault, method, args, gas, deposit)
            .await
            .map(|_| ())
            .map_err(ErrorWrapper::from)
    }

    async fn vault_call(&self, method: &str, args: impl Serialize) -> Result<(), ErrorWrapper> {
        self.vault_call_with(method, args, None, None).await
    }

    async fn vault_call_returning<T: DeserializeOwned>(
        &self,
        method: &str,
        args: impl Serialize,
        gas: Option<Gas>,
        deposit: Option<u128>,
    ) -> Result<T, ErrorWrapper> {
        let status = self
            .call(&self.vault, method, args, gas, deposit)
            .await
            .map_err(ErrorWrapper::from)?;

        let FinalExecutionStatus::SuccessValue(bytes) = status else {
            return Err(ErrorWrapper::Wrapped(
                "Transaction returned no value".to_string(),
            ));
        };

        serde_json::from_slice(&bytes).map_err(ErrorWrapper::from)
    }
}

fn should_retry(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.is::<tokio::time::error::Elapsed>() {
            return true;
        }
        if cause.is::<std::io::Error>() {
            return true;
        }
    }
    false
}

// Generate common vault methods via macro
crate::impl_vault_methods!(KeyPoolClient);

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::KeyType;

    #[test]
    fn key_pool_config_default_values() {
        let config = KeyPoolConfig::default();
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.max_nonce_retries, 3);
        assert_eq!(config.block_hash_ttl_seconds, 30);
        assert_eq!(config.view_cache_capacity, 100);
        assert_eq!(config.view_cache_ttl_seconds, 5);
        assert!(config.retry.is_some());
    }

    #[test]
    fn key_credential_to_signer_valid() {
        let secret_key = near_crypto::SecretKey::from_random(KeyType::ED25519);
        let cred = KeyCredential {
            account_id: AccountId::from("test.near".to_string()),
            secret_key: secret_key.to_string(),
        };
        assert!(cred.into_signer().is_ok());
    }

    #[test]
    fn key_credential_to_signer_invalid_account() {
        let secret_key = near_crypto::SecretKey::from_random(KeyType::ED25519);
        let cred = KeyCredential {
            account_id: AccountId::from("invalid account!!!".to_string()),
            secret_key: secret_key.to_string(),
        };
        assert!(cred.into_signer().is_err());
    }

    #[test]
    fn key_credential_to_signer_invalid_key() {
        let cred = KeyCredential {
            account_id: AccountId::from("test.near".to_string()),
            secret_key: "not-a-valid-key".to_string(),
        };
        assert!(cred.into_signer().is_err());
    }

    #[test]
    fn retry_config_normalized_min_values() {
        let config = RetryConfig {
            max_attempts: 0,
            initial_backoff_ms: 0,
            max_backoff_ms: 0,
        };
        let normalized = config.normalized();
        assert_eq!(normalized.max_attempts, 1);
        assert_eq!(normalized.initial_backoff_ms, 1);
        assert_eq!(normalized.max_backoff_ms, 1);
    }

    #[test]
    fn is_invalid_nonce_error_detects_nonce() {
        let err = anyhow::anyhow!("InvalidNonce: expected 5, got 4");
        assert!(KeyPoolClient::is_invalid_nonce_error(&err));

        let err = anyhow::anyhow!("invalid nonce");
        assert!(KeyPoolClient::is_invalid_nonce_error(&err));

        let err = anyhow::anyhow!("Some other error");
        assert!(!KeyPoolClient::is_invalid_nonce_error(&err));
    }

    #[test]
    fn parse_u128_plain_and_json() {
        assert_eq!(crate::parse_u128("123").unwrap(), 123);
        assert_eq!(crate::parse_u128("\"456\"").unwrap(), 456);
    }
}
