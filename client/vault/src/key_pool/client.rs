use std::{
    str::FromStr,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use near_account_id::AccountId as NearAccountId;
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::{
    auth::ApiKey,
    methods::{
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient,
};
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::Gas,
    views::{FinalExecutionStatus, TxExecutionStatus},
};
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, instrument, warn};
use zeroize::Zeroize;

use crate::{
    parse_account_id, view_core, AccountId, AllocationDelta, CapGroupUpdate, CapGroupUpdateKey,
    ErrorWrapper, FeeAccrualAnchor, Fees, ForeignU128, MarketId, RealAssetsReport, Restrictions,
    ResyncIdleReport, RetryConfig, TimelockKind, VaultConfiguration, ViewCache, DEFAULT_GAS,
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

/// Configuration for `KeyPoolClient`.
#[derive(uniffi::Record, Clone)]
pub struct KeyPoolConfig {
    /// Default timeout for RPC calls in seconds.
    pub timeout_seconds: u64,

    /// Retry configuration for transient errors.
    pub retry: Option<RetryConfig>,

    /// Maximum nonce retry attempts specifically for `InvalidNonce` errors.
    pub max_nonce_retries: u32,

    /// Block hash TTL in seconds (for key slot nonce caching).
    pub block_hash_ttl_seconds: u64,

    /// View cache capacity (0 = disabled).
    pub view_cache_capacity: u32,

    /// View cache TTL in seconds.
    pub view_cache_ttl_seconds: u64,

    /// Optional RPC API key for authenticated endpoints (e.g., `FastNEAR`).
    pub rpc_api_key: Option<String>,
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
            rpc_api_key: None,
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

        let inner = {
            let client = JsonRpcClient::connect(rpc_url);
            if let Some(api_key) = &config.rpc_api_key {
                let api_key =
                    ApiKey::new(api_key).map_err(|e| ErrorWrapper::Wrapped(e.to_string()))?;
                client.header(api_key)
            } else {
                client
            }
        };
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

        let view_cache = view_core::build_view_cache(&config);

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
        view_core::view_with_cache(
            &self.inner,
            &self.config,
            &self.view_cache,
            account_id,
            function_name,
            args,
        )
        .await
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
            let slot = self.pool.select().map_err(|e| anyhow::anyhow!("{e}"))?;
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
                        bail!("Transaction failed: {tx_err:?}");
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
        let mut attempts_left = retry.map_or(1, |r| r.max_attempts);
        let mut backoff_ms = retry.map_or(0, |r| r.initial_backoff_ms);

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
            bail!("No outcome {tx_hash}");
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
            bail!("No outcome {tx_hash}");
        };

        let status = outcome.into_outcome().status;
        Ok(status)
    }

    /// Check if an error indicates an `InvalidNonce` condition.
    fn is_invalid_nonce_error(err: &anyhow::Error) -> bool {
        let err_str = err.to_string();
        err_str.contains("InvalidNonce") || err_str.contains("invalid nonce")
    }

    /// Check RPC error for `InvalidNonce`.
    fn is_rpc_invalid_nonce(
        err: &near_jsonrpc_client::errors::JsonRpcError<RpcTransactionError>,
    ) -> bool {
        if let Some(handler_err) = err.handler_error() {
            let err_str = format!("{handler_err:?}");
            return err_str.contains("InvalidNonce") || err_str.contains("invalid nonce");
        }
        false
    }

    #[inline]
    #[allow(clippy::unused_self)]
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

crate::impl_view_cache_methods!(KeyPoolClient);
crate::impl_vault_view_methods!(KeyPoolClient);
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
