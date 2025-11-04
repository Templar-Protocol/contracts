// SPDX-License-Identifier: MIT
//! Swap provider implementations for liquidation operations.
//!
//! This module provides a flexible, extensible architecture for integrating
//! different swap/exchange protocols (Ref Finance, 1-Click API, etc.) used
//! during liquidation operations.
//!
//! # Architecture
//!
//! The module follows the Strategy pattern to allow runtime selection of swap
//! providers while maintaining a consistent interface. This enables:
//! - Easy addition of new swap providers without modifying existing code
//! - Testability through mock implementations
//! - Type-safe asset handling across different token standards (NEP-141, NEP-245)
//!
//! # Example
//!
//! ```no_run
//! use templar_bots::swap::{SwapProvider, RefSwap};
//! use near_jsonrpc_client::JsonRpcClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
//! let swap_provider = RefSwap::new(
//!     "v2.ref-finance.near".parse()?,
//!     client,
//!     signer,
//! );
//!
//! // Get quote
//! let quote = swap_provider.quote(&from_asset, &to_asset, output_amount).await?;
//!
//! // Execute swap
//! let result = swap_provider.swap(&from_asset, &to_asset, quote).await?;
//! # Ok(())
//! # }
//! ```

pub mod oneclick;
pub mod provider;
pub mod r#ref;

// Re-export for convenience
pub use oneclick::OneClickSwap;
pub use provider::SwapProviderImpl;
pub use r#ref::RefSwap;

use near_primitives::views::FinalExecutionStatus;
use near_sdk::{json_types::U128, AccountId};
use templar_common::asset::{AssetClass, FungibleAsset};

use crate::rpc::AppResult;

/// Core trait for swap provider implementations.
///
/// This trait defines the interface that all swap providers must implement,
/// enabling polymorphic usage of different DEX protocols.
///
/// # Type Safety
///
/// The trait uses generic `AssetClass` bounds to ensure compile-time type safety
/// when working with different asset types (collateral vs borrow assets).
///
/// # Object Safety
///
/// This trait is object-safe, allowing for dynamic dispatch via `Box<dyn SwapProvider>`.
#[async_trait::async_trait]
pub trait SwapProvider: Send + Sync {
    /// Quotes the input amount needed to obtain a specific output amount.
    ///
    /// # Arguments
    ///
    /// * `from_asset` - The asset to swap from
    /// * `to_asset` - The asset to swap to
    /// * `output_amount` - The desired output amount
    ///
    /// # Returns
    ///
    /// The input amount required to obtain the desired output amount,
    /// including slippage and fees.
    ///
    /// # Errors
    ///
    /// Returns `AppError` if:
    /// - The asset pair is not supported
    /// - The liquidity is insufficient
    /// - The RPC call fails
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128>;

    /// Executes a swap operation.
    ///
    /// # Arguments
    ///
    /// * `from_asset` - The asset to swap from
    /// * `to_asset` - The asset to swap to
    /// * `amount` - The input amount to swap
    ///
    /// # Returns
    ///
    /// The final execution status of the swap transaction.
    ///
    /// # Errors
    ///
    /// Returns `AppError` if:
    /// - The transaction fails to execute
    /// - The slippage exceeds acceptable limits
    /// - The deadline is exceeded
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus>;

    /// Returns the name of the swap provider for logging and debugging.
    fn provider_name(&self) -> &'static str;

    /// Returns a debug representation of the provider.
    fn debug_name(&self) -> String {
        self.provider_name().to_string()
    }

    /// Checks if the provider supports a given asset pair.
    ///
    /// # Default Implementation
    ///
    /// The default implementation returns `true` for all pairs. Providers
    /// should override this if they have specific asset restrictions.
    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        _from_asset: &FungibleAsset<F>,
        _to_asset: &FungibleAsset<T>,
    ) -> bool {
        true
    }

    /// Ensures an account is registered with a token contract's storage.
    ///
    /// This method calls `storage_deposit` on the token contract to register
    /// the account before it can receive tokens. This is required by NEP-141.
    ///
    /// # Arguments
    ///
    /// * `token_contract` - The token contract to register with
    /// * `account_id` - The account to register
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if registration succeeds or the account is already registered.
    ///
    /// # Errors
    ///
    /// Returns `AppError` if the registration transaction fails.
    async fn ensure_storage_registration<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()>;
}
