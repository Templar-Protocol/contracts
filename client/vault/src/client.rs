//! Canonical vault client for high-concurrency use.
//!
//! This is the preferred UniFFI surface. It supports:
//! - multi-key transaction submission (nonce-safe)
//! - view caching
//! - retry/backoff
//!
//! Internally it is backed by [`KeyPoolClient`].

use anyhow::Result;
use near_account_id::AccountId as NearAccountId;
use near_primitives::types::Gas;
use near_primitives::views::FinalExecutionStatus;
use near_sdk::json_types::{U128, U64};
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

#[allow(unused_imports)]
use crate::{
    parse_account_id, AccountId, AllocationDelta, CapGroupUpdate, CapGroupUpdateKey, ErrorWrapper,
    FeeAccrualAnchor, Fees, ForeignU128, KeyCredential, KeyPoolClient, KeyPoolConfig, MarketId,
    PoolHealth, RealAssetsReport, Restrictions, RetryConfig, TimelockKind, VaultConfiguration,
};

/// Configuration for [`VaultClient`].
///
/// Defaults are tuned for high-concurrency / service-style usage.
#[derive(uniffi::Record, Clone)]
pub struct VaultClientConfig {
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

impl Default for VaultClientConfig {
    fn default() -> Self {
        // Hedgefund/service defaults:
        // - retries enabled
        // - moderately aggressive nonce retries (multi-key concurrency)
        // - view cache enabled but short TTL
        Self {
            timeout_seconds: 60,
            retry: Some(RetryConfig {
                max_attempts: 5,
                initial_backoff_ms: 100,
                max_backoff_ms: 5_000,
            }),
            max_nonce_retries: 5,
            block_hash_ttl_seconds: 30,
            view_cache_capacity: 2_000,
            view_cache_ttl_seconds: 2,
        }
    }
}

impl From<VaultClientConfig> for KeyPoolConfig {
    fn from(value: VaultClientConfig) -> Self {
        Self {
            timeout_seconds: value.timeout_seconds,
            retry: value.retry,
            max_nonce_retries: value.max_nonce_retries,
            block_hash_ttl_seconds: value.block_hash_ttl_seconds,
            view_cache_capacity: value.view_cache_capacity,
            view_cache_ttl_seconds: value.view_cache_ttl_seconds,
        }
    }
}

#[derive(uniffi::Object)]
pub struct VaultClient {
    // Required by impl_vault_methods!
    vault: NearAccountId,

    // Actual implementation.
    inner: KeyPoolClient,
}

#[uniffi::export(async_runtime = "tokio")]
impl VaultClient {
    #[uniffi::constructor]
    #[instrument(skip(credentials), fields(rpc_url = %rpc_url))]
    pub fn new_key_pool_default(
        rpc_url: String,
        vault: &AccountId,
        credentials: Vec<KeyCredential>,
    ) -> Result<Self, ErrorWrapper> {
        Self::new_key_pool(rpc_url, vault, credentials, VaultClientConfig::default())
    }

    #[uniffi::constructor]
    #[instrument(skip(credential), fields(rpc_url = %rpc_url))]
    pub fn new_single_key_default(
        rpc_url: String,
        vault: &AccountId,
        credential: KeyCredential,
    ) -> Result<Self, ErrorWrapper> {
        Self::new_single_key(rpc_url, vault, credential, VaultClientConfig::default())
    }


    #[uniffi::constructor]
    #[instrument(skip(credentials, config), fields(rpc_url = %rpc_url))]
    pub fn new_key_pool(
        rpc_url: String,
        vault: &AccountId,
        credentials: Vec<KeyCredential>,
        config: VaultClientConfig,
    ) -> Result<Self, ErrorWrapper> {
        let vault_id = parse_account_id(vault)?;
        let inner = KeyPoolClient::new(rpc_url, vault, credentials, config.into())?;
        Ok(Self {
            vault: vault_id,
            inner,
        })
    }

    #[uniffi::constructor]
    #[instrument(skip(credential, config), fields(rpc_url = %rpc_url))]
    pub fn new_single_key(
        rpc_url: String,
        vault: &AccountId,
        credential: KeyCredential,
        config: VaultClientConfig,
    ) -> Result<Self, ErrorWrapper> {
        Self::new_key_pool(rpc_url, vault, vec![credential], config)
    }

    /// Get the vault account ID.
    pub fn vault_account(&self) -> AccountId {
        self.inner.vault_account()
    }

    /// Get health status of the underlying key pool.
    pub fn get_pool_health(&self) -> PoolHealth {
        self.inner.get_pool_health()
    }

    /// Enable view cache.
    pub fn enable_view_cache(&self, capacity: u32, ttl_seconds: u64) {
        self.inner.enable_view_cache(capacity, ttl_seconds)
    }

    /// Disable view cache.
    pub fn disable_view_cache(&self) {
        self.inner.disable_view_cache()
    }

    /// Clear view cache.
    pub async fn clear_view_cache(&self) -> Result<(), ErrorWrapper> {
        self.inner.clear_view_cache().await
    }

    // ---------------------------------------------------------------------
    // Complex methods not covered by impl_vault_methods!
    // ---------------------------------------------------------------------

    pub async fn get_cap_groups(&self) -> Result<Vec<crate::CapGroup>, ErrorWrapper> {
        self.inner.get_cap_groups().await
    }

    pub async fn get_pending_governance_actions(
        &self,
    ) -> Result<Vec<crate::PendingGovernanceAction>, ErrorWrapper> {
        self.inner.get_pending_governance_actions().await
    }

    pub async fn get_market_id_of_account(
        &self,
        market: &AccountId,
    ) -> Result<Option<crate::MarketId>, ErrorWrapper> {
        self.inner.get_market_id_of_account(market).await
    }

    pub async fn get_market_account_by_id(
        &self,
        market_id: crate::MarketId,
    ) -> Result<Option<AccountId>, ErrorWrapper> {
        self.inner.get_market_account_by_id(market_id).await
    }

    pub async fn list_markets_with_ids(&self) -> Result<Vec<crate::MarketWithId>, ErrorWrapper> {
        self.inner.list_markets_with_ids().await
    }

    pub async fn get_vault_snapshot(&self) -> Result<crate::VaultSnapshot, ErrorWrapper> {
        self.inner.get_vault_snapshot().await
    }

    pub async fn resolve_market_ids(
        &self,
        markets: &[AccountId],
    ) -> Result<Vec<Option<crate::MarketId>>, ErrorWrapper> {
        self.inner.resolve_market_ids(markets).await
    }

    pub async fn resolve_market_accounts(
        &self,
        market_ids: &[crate::MarketId],
    ) -> Result<Vec<Option<AccountId>>, ErrorWrapper> {
        self.inner.resolve_market_accounts(market_ids).await
    }

    pub async fn refresh_all_markets(&self) -> Result<crate::RealAssetsReport, ErrorWrapper> {
        self.inner.refresh_all_markets().await
    }
}

// -------------------------------------------------------------------------
// Helper methods required by impl_vault_methods!
// -------------------------------------------------------------------------

impl VaultClient {
    async fn vault_view_u128(
        &self,
        method: &str,
        args: impl Serialize,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let u = self
            .inner
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
        self.inner
            .call(&self.vault, method, args, gas, deposit)
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
            .inner
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

    #[instrument(skip(self, args), fields(account_id = %account_id, method = function_name))]
    async fn view<T: DeserializeOwned>(
        &self,
        account_id: &NearAccountId,
        function_name: &str,
        args: impl Serialize,
    ) -> Result<T> {
        self.inner.view(account_id, function_name, args).await
    }

    #[inline]
    fn near_id(&self, id: &AccountId) -> Result<NearAccountId, ErrorWrapper> {
        parse_account_id(id)
    }
}

// Generate common vault methods via macro
crate::impl_vault_methods!(VaultClient);
