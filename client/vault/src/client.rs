//! Canonical vault client for high-concurrency use.
//!
//! This is the preferred UniFFI surface. It supports:
//! - multi-key transaction submission (nonce-safe)
//! - view caching
//! - retry/backoff
//!
//! Internally it is backed by [`KeyPoolClient`].
//!
//! # Migration from Legacy `Client`
//!
//! The legacy `Client` has been removed. Use `VaultClient` instead:
//!
//! ## Single-key usage (equivalent to old `Client::new_client`)
//!
//! ```ignore
//! // Old:
//! let client = Client::new_client(rpc_url, &signer_account, &signer_key, &vault, timeout)?;
//!
//! // New:
//! let credential = KeyCredential {
//!     account_id: signer_account,
//!     secret_key: signer_key.to_string(),
//! };
//! let client = VaultClient::new_single_key_default(rpc_url, &vault, credential)?;
//! ```
//!
//! ## With custom timeout and retry config
//!
//! ```ignore
//! // Old:
//! let client = Client::new_client_with_retry(rpc_url, &signer_account, &signer_key, &vault, timeout, retry)?;
//!
//! // New:
//! let credential = KeyCredential {
//!     account_id: signer_account,
//!     secret_key: signer_key.to_string(),
//! };
//! let config = VaultClientConfig {
//!     timeout_seconds: timeout,
//!     retry: Some(retry),
//!     ..VaultClientConfig::default()
//! };
//! let client = VaultClient::new_single_key(rpc_url, &vault, credential, config)?;
//! ```
//!
//! ## Key differences
//!
//! - `VaultClient` uses `KeyCredential` struct instead of separate account/key parameters
//! - Configuration is bundled in `VaultClientConfig` with sensible defaults
//! - View cache is enabled by default (set `view_cache_capacity: 0` to disable)
//! - `VaultClient` supports multi-key pools for high-concurrency scenarios

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
    FeeAccrualAnchor, Fees, ForeignU128, IdleResyncOutcome, KeyCredential, KeyPoolClient,
    KeyPoolConfig, MarketId, PoolHealth, RealAssetsReport, Restrictions, ResyncIdleReport,
    RetryConfig, TimelockKind, VaultConfiguration,
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

    /// Optional RPC API key for authenticated endpoints (e.g., FastNEAR).
    pub rpc_api_key: Option<String>,
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
            rpc_api_key: None,
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
            rpc_api_key: value.rpc_api_key,
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

    /// Refresh all markets and return the real assets report.
    pub async fn refresh_all_markets(&self) -> Result<crate::RealAssetsReport, ErrorWrapper> {
        self.inner.refresh_all_markets().await
    }

    /// Transfer fungible tokens to the vault via ft_transfer_call.
    ///
    /// This is the standard way to deposit tokens into the vault. The vault will
    /// mint shares to the sender based on the deposited amount.
    ///
    /// # Arguments
    /// * `token` - The token contract account ID (e.g., "usdt.fakes.testnet")
    /// * `amount` - Amount of tokens to transfer (as string, e.g., "1000000" for 1 USDT)
    /// * `msg` - Optional message for the vault (defaults to "Supply" for standard deposit)
    ///
    /// # Returns
    /// The amount of tokens actually used by the vault (computed as `amount - unused`).
    ///
    /// # Note
    /// Per NEP-141, ft_on_transfer returns the *unused* amount to refund. We compute
    /// `used = amount - unused` here. However, this value should not be fully trusted
    /// for accounting—verify via balance changes instead.
    #[instrument(skip(self, token, amount, msg))]
    pub async fn ft_transfer_call(
        &self,
        token: &AccountId,
        amount: &ForeignU128,
        msg: Option<String>,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let token_id = parse_account_id(token)?;
        let amount_u128 = crate::parse_u128(amount)?;

        // NEP-141 ft_transfer_call args
        #[derive(serde::Serialize)]
        struct FtTransferCallArgs {
            receiver_id: near_account_id::AccountId,
            amount: near_sdk::json_types::U128,
            memo: Option<String>,
            msg: String,
        }

        // Default to DepositMsg::Supply for standard vault deposits
        let msg = msg.unwrap_or_else(|| {
            serde_json::to_string(&templar_common::vault::DepositMsg::Supply)
                .expect("DepositMsg serialization cannot fail")
        });

        let args = FtTransferCallArgs {
            receiver_id: self.vault.clone(),
            amount: near_sdk::json_types::U128(amount_u128),
            memo: None,
            msg,
        };

        // ft_transfer_call requires exactly 1 yoctoNEAR deposit
        // Use high gas limit for cross-contract call
        const FT_TRANSFER_GAS: Gas = 100_000_000_000_000; // 100 TGas

        let status = self
            .inner
            .call(
                &token_id,
                "ft_transfer_call",
                args,
                Some(FT_TRANSFER_GAS),
                Some(1),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        match status {
            FinalExecutionStatus::SuccessValue(bytes) => {
                // Per NEP-141, ft_on_transfer returns U128 of *unused* tokens (to refund).
                // We compute used = amount - unused.
                let unused: near_sdk::json_types::U128 =
                    serde_json::from_slice(&bytes).map_err(ErrorWrapper::from)?;
                let used = amount_u128.saturating_sub(unused.0);
                Ok(used.to_string())
            }
            FinalExecutionStatus::Failure(err) => Err(ErrorWrapper::Wrapped(format!(
                "ft_transfer_call failed: {:?}",
                err
            ))),
            _ => Err(ErrorWrapper::Wrapped(
                "Unexpected execution status".to_string(),
            )),
        }
    }

    /// Transfer fungible tokens directly to the vault via `ft_transfer`.
    ///
    /// This is a "donation" from the vault's perspective (no receiver hook), so it will NOT
    /// update the vault's stored `idle_balance` until you call `refresh_idle_balance()`.
    #[instrument(skip(self, token, amount, memo))]
    pub async fn ft_transfer(
        &self,
        token: &AccountId,
        amount: &ForeignU128,
        memo: Option<String>,
    ) -> Result<(), ErrorWrapper> {
        let token_id = parse_account_id(token)?;
        let amount_u128 = crate::parse_u128(amount)?;

        #[derive(serde::Serialize)]
        struct Args {
            receiver_id: near_account_id::AccountId,
            amount: near_sdk::json_types::U128,
            memo: Option<String>,
        }

        // ft_transfer requires exactly 1 yoctoNEAR deposit
        const FT_TRANSFER_GAS: Gas = 30_000_000_000_000; // 30 TGas

        let status = self
            .inner
            .call(
                &token_id,
                "ft_transfer",
                Args {
                    receiver_id: self.vault.clone(),
                    amount: near_sdk::json_types::U128(amount_u128),
                    memo,
                },
                Some(FT_TRANSFER_GAS),
                Some(1),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        match status {
            FinalExecutionStatus::SuccessValue(_) => Ok(()),
            FinalExecutionStatus::Failure(err) => Err(ErrorWrapper::Wrapped(format!(
                "ft_transfer failed: {:?}",
                err
            ))),
            _ => Err(ErrorWrapper::Wrapped(
                "Unexpected execution status".to_string(),
            )),
        }
    }

    /// Read `ft_balance_of(account_id)` from an arbitrary NEP-141 token.
    #[instrument(skip(self, token, account_id))]
    pub async fn ft_balance_of(
        &self,
        token: &AccountId,
        account_id: &AccountId,
    ) -> Result<ForeignU128, ErrorWrapper> {
        let token_id = parse_account_id(token)?;
        let account_id = parse_account_id(account_id)?;

        #[derive(serde::Serialize)]
        struct Args {
            account_id: near_account_id::AccountId,
        }

        let balance: U128 = self
            .inner
            .view(&token_id, "ft_balance_of", Args { account_id })
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(balance.0.to_string())
    }

    // =========================================================================
    // NEP-145 Storage Management
    // =========================================================================

    /// Get storage balance bounds for the vault contract (NEP-145).
    ///
    /// Returns the minimum and optional maximum storage deposit required.
    #[instrument(skip(self))]
    pub async fn storage_balance_bounds(
        &self,
    ) -> Result<crate::StorageBalanceBounds, ErrorWrapper> {
        #[derive(serde::Deserialize)]
        struct Bounds {
            min: U128,
            max: Option<U128>,
        }

        let bounds: Bounds = self
            .inner
            .view(&self.vault, "storage_balance_bounds", ())
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(crate::StorageBalanceBounds {
            min: bounds.min.0.to_string(),
            max: bounds.max.map(|m| m.0.to_string()),
        })
    }

    /// Get storage balance for an account on the vault contract (NEP-145).
    ///
    /// Returns None if the account is not registered.
    #[instrument(skip(self))]
    pub async fn storage_balance_of(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<crate::StorageBalance>, ErrorWrapper> {
        let vault: AccountId = self.vault.to_string().into();
        self.token_storage_balance_of(&vault, account_id).await
    }

    /// Register account with storage deposit on the vault contract (NEP-145).
    ///
    /// # Arguments
    /// * `account_id` - Account to register. If None, registers the sender.
    /// * `deposit_yocto` - Amount of NEAR to deposit for storage (in yoctoNEAR).
    ///
    /// # Returns
    /// The resulting storage balance after deposit.
    #[instrument(skip(self))]
    pub async fn storage_deposit(
        &self,
        account_id: Option<AccountId>,
        deposit_yocto: &ForeignU128,
    ) -> Result<crate::StorageBalance, ErrorWrapper> {
        #[derive(serde::Serialize)]
        struct Args {
            account_id: Option<near_account_id::AccountId>,
            registration_only: Option<bool>,
        }

        #[derive(serde::Deserialize)]
        struct Balance {
            total: U128,
            available: U128,
        }

        let account = match account_id {
            Some(ref id) => Some(parse_account_id(id)?),
            None => None,
        };
        let deposit = crate::parse_u128(deposit_yocto)?;

        const STORAGE_DEPOSIT_GAS: Gas = 30_000_000_000_000; // 30 TGas

        let status = self
            .inner
            .call(
                &self.vault,
                "storage_deposit",
                Args {
                    account_id: account,
                    registration_only: Some(true),
                },
                Some(STORAGE_DEPOSIT_GAS),
                Some(deposit),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        match status {
            FinalExecutionStatus::SuccessValue(bytes) => {
                let balance: Balance =
                    serde_json::from_slice(&bytes).map_err(ErrorWrapper::from)?;
                Ok(crate::StorageBalance {
                    total: balance.total.0.to_string(),
                    available: balance.available.0.to_string(),
                })
            }
            FinalExecutionStatus::Failure(err) => Err(ErrorWrapper::Wrapped(format!(
                "storage_deposit failed: {:?}",
                err
            ))),
            _ => Err(ErrorWrapper::Wrapped(
                "Unexpected execution status".to_string(),
            )),
        }
    }

    /// Get storage balance for an account on a token/market contract (NEP-145).
    ///
    /// Returns None if the account is not registered.
    #[instrument(skip(self))]
    pub async fn token_storage_balance_of(
        &self,
        token: &AccountId,
        account_id: &AccountId,
    ) -> Result<Option<crate::StorageBalance>, ErrorWrapper> {
        self.storage_balance_of_on(token, account_id).await
    }

    /// NEP-145 `storage_balance_of` against an arbitrary contract.
    ///
    /// Returns None if the account is not registered.
    #[instrument(skip(self))]
    pub async fn storage_balance_of_on(
        &self,
        contract_id: &AccountId,
        account_id: &AccountId,
    ) -> Result<Option<crate::StorageBalance>, ErrorWrapper> {
        #[derive(serde::Serialize)]
        struct Args {
            account_id: near_account_id::AccountId,
        }

        #[derive(serde::Deserialize)]
        struct Balance {
            total: U128,
            available: U128,
        }

        let contract_id = parse_account_id(contract_id)?;
        let account = parse_account_id(account_id)?;
        let balance: Option<Balance> = self
            .inner
            .view(
                &contract_id,
                "storage_balance_of",
                Args {
                    account_id: account,
                },
            )
            .await
            .map_err(ErrorWrapper::from)?;

        Ok(balance.map(|b| crate::StorageBalance {
            total: b.total.0.to_string(),
            available: b.available.0.to_string(),
        }))
    }

    /// Register account with storage deposit on a token contract (NEP-145).
    ///
    /// This is needed before `ft_transfer_call` to ensure the receiver (vault)
    /// is registered with the token contract.
    ///
    /// # Arguments
    /// * `token` - The token contract account ID.
    /// * `account_id` - Account to register. If None, registers the sender.
    /// * `deposit_yocto` - Amount of NEAR to deposit for storage (in yoctoNEAR).
    ///                     Typical value is "1250000000000000000000" (0.00125 NEAR).
    ///
    /// # Returns
    /// The resulting storage balance after deposit.
    #[instrument(skip(self))]
    pub async fn token_storage_deposit(
        &self,
        token: &AccountId,
        account_id: Option<AccountId>,
        deposit_yocto: &ForeignU128,
    ) -> Result<crate::StorageBalance, ErrorWrapper> {
        self.storage_deposit_on(token, account_id, deposit_yocto)
            .await
    }

    /// NEP-145 `storage_deposit` against an arbitrary contract.
    #[instrument(skip(self))]
    pub async fn storage_deposit_on(
        &self,
        contract_id: &AccountId,
        account_id: Option<AccountId>,
        deposit_yocto: &ForeignU128,
    ) -> Result<crate::StorageBalance, ErrorWrapper> {
        #[derive(serde::Serialize)]
        struct Args {
            account_id: Option<near_account_id::AccountId>,
            registration_only: Option<bool>,
        }

        #[derive(serde::Deserialize)]
        struct Balance {
            total: U128,
            available: U128,
        }

        let contract_id = parse_account_id(contract_id)?;
        let account = match account_id {
            Some(ref id) => Some(parse_account_id(id)?),
            None => None,
        };
        let deposit = crate::parse_u128(deposit_yocto)?;

        const STORAGE_DEPOSIT_GAS: Gas = 30_000_000_000_000; // 30 TGas

        let status = self
            .inner
            .call(
                &contract_id,
                "storage_deposit",
                Args {
                    account_id: account,
                    registration_only: Some(true),
                },
                Some(STORAGE_DEPOSIT_GAS),
                Some(deposit),
            )
            .await
            .map_err(ErrorWrapper::from)?;

        match status {
            FinalExecutionStatus::SuccessValue(bytes) => {
                let balance: Balance =
                    serde_json::from_slice(&bytes).map_err(ErrorWrapper::from)?;
                Ok(crate::StorageBalance {
                    total: balance.total.0.to_string(),
                    available: balance.available.0.to_string(),
                })
            }
            FinalExecutionStatus::Failure(err) => Err(ErrorWrapper::Wrapped(format!(
                "storage_deposit_on failed: {:?}",
                err
            ))),
            _ => Err(ErrorWrapper::Wrapped(
                "Unexpected execution status".to_string(),
            )),
        }
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

// Generate complex view methods via macro
crate::impl_vault_view_methods!(VaultClient);

// Generate common vault methods via macro
crate::impl_vault_methods!(VaultClient);
