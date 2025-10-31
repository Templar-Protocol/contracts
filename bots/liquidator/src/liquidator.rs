// SPDX-License-Identifier: MIT
//! Production-grade liquidator bot with extensible modular architecture.
//!
//! This module provides a modern liquidator implementation with:
//! - Inventory-based liquidation (no pre-liquidation swaps)
//! - Modular architecture with focused components
//! - Strategy pattern for flexible liquidation approaches
//! - Comprehensive error handling and logging
//! - Gas cost estimation and profitability analysis
//!
//! # Architecture
//!
//! The liquidator is structured into focused modules:
//! - `service`: Bot lifecycle management (registry, inventory, liquidation rounds)
//! - `scanner`: Market position scanning and version compatibility
//! - `executor`: Transaction creation and execution
//! - `oracle`: Price fetching from various oracle types
//! - `profitability`: Cost/profit calculations
//! - `inventory`: Asset balance tracking and management
//! - `strategy`: Liquidation amount calculations
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use templar_liquidator::ServiceConfig, LiquidatorService};
//! use templar_liquidator::liquidation_strategy::PartialLiquidationStrategy;
//! use templar_liquidator::CollateralStrategy;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let strategy = Arc::new(PartialLiquidationStrategy::new(50, 50, 10));
//!
//! let config = ServiceConfig {
//!     registries: vec![],
//!     signer_key: todo!(),
//!     signer_account: todo!(),
//!     network: templar_liquidator::rpc::Network::Testnet,
//!     rpc_url: None,
//!     transaction_timeout: 60,
//!     liquidation_scan_interval: 600,
//!     registry_refresh_interval: 3600,
//!     inventory_refresh_interval: 300,
//!     concurrency: 10,
//!     strategy,
//!     collateral_strategy: CollateralStrategy::Hold,
//!     dry_run: false,
//! };
//!
//! let service = LiquidatorService::new(config);
//! service.run().await;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{json_types::U128, AccountId};
use templar_common::{
    asset::{CollateralAsset, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    market::MarketConfiguration,
    oracle::pyth::OracleResponse,
};
use tracing::{debug, info};

use crate::liquidation_strategy::LiquidationStrategy;

// Modules
pub mod config;
pub mod executor;
pub mod inventory;
pub mod liquidation_strategy;
pub mod oracle;
pub mod profitability;
pub mod rpc;
pub mod scanner;
pub mod service;
pub mod swap;

// Re-exports for convenience
pub use config::Args;
pub use executor::LiquidationExecutor;
pub use inventory::InventoryManager;
pub use oracle::OracleFetcher;
pub use profitability::ProfitabilityCalculator;
pub use scanner::MarketScanner;
pub use service::{LiquidatorService, ServiceConfig};

// Error conversions
use crate::rpc::AppError;

impl From<AppError> for LiquidatorError {
    fn from(err: AppError) -> Self {
        LiquidatorError::SwapProviderError(err)
    }
}

impl From<inventory::InventoryError> for LiquidatorError {
    fn from(err: inventory::InventoryError) -> Self {
        match err {
            inventory::InventoryError::InsufficientBalance { .. } => {
                LiquidatorError::InsufficientBalance
            }
            _ => LiquidatorError::StrategyError(err.to_string()),
        }
    }
}

/// Result of a liquidation attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidationOutcome {
    /// Position was successfully liquidated
    Liquidated,
    /// Position is healthy and not liquidatable
    NotLiquidatable,
    /// Position is liquidatable but unprofitable
    Unprofitable,
}

/// Errors that can occur during liquidation operations.
#[derive(Debug, thiserror::Error)]
pub enum LiquidatorError {
    #[error("Failed to fetch borrow status: {0}")]
    FetchBorrowStatus(rpc::RpcError),
    #[error("Failed to serialize data: {0}")]
    SerializeError(#[from] near_sdk::serde_json::Error),
    #[error("Price pair retrieval error: {0}")]
    PricePairError(#[from] templar_common::market::error::RetrievalError),
    #[error("Swap provider error: {0}")]
    SwapProviderError(AppError),
    #[error("Failed to get market configuration: {0}")]
    GetConfigurationError(rpc::RpcError),
    #[error("Failed to fetch oracle prices: {0}")]
    PriceFetchError(rpc::RpcError),
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(rpc::RpcError),
    #[error("Liquidation transaction error: {0}")]
    LiquidationTransactionError(rpc::RpcError),
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),
    #[error("Failed to list borrow positions: {0}")]
    ListBorrowPositionsError(rpc::RpcError),
    #[error("Failed to fetch balance: {0}")]
    FetchBalanceError(rpc::RpcError),
    #[error("Failed to list deployments: {0}")]
    ListDeploymentsError(rpc::RpcError),
    #[error("Strategy error: {0}")]
    StrategyError(String),
    #[error("Insufficient balance for liquidation")]
    InsufficientBalance,
}

pub type LiquidatorResult<T = ()> = Result<T, LiquidatorError>;

/// Collateral management strategy
#[derive(Debug, Clone)]
pub enum CollateralStrategy {
    /// Hold collateral as received (default)
    Hold,
    /// Swap collateral to a primary asset (e.g., USDC)
    SwapToPrimary {
        /// Primary asset to swap to
        primary_asset: FungibleAsset<CollateralAsset>,
    },
    /// Swap collateral back to borrow assets (assets used for liquidations)
    SwapToBorrow,
}

/// Production-grade liquidator with modular architecture.
///
/// This liquidator orchestrates specialized modules:
/// - Scanner: Fetches and evaluates borrow positions
/// - Oracle: Fetches price data
/// - Profitability: Calculates costs and profits
/// - Executor: Executes liquidation transactions
/// - Inventory: Manages asset balances
pub struct Liquidator {
    /// Market scanner for position fetching
    scanner: scanner::MarketScanner,
    /// Oracle fetcher for price data
    oracle_fetcher: oracle::OracleFetcher,
    /// Liquidation executor
    executor: executor::LiquidationExecutor,
    /// Market contract to liquidate positions in
    pub market: AccountId,
    /// Market configuration (cached)
    market_config: MarketConfiguration,
    /// Liquidation strategy
    strategy: Arc<dyn LiquidationStrategy>,
}

impl Liquidator {
    /// Creates a new liquidator instance.
    ///
    /// # Arguments
    ///
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    /// * `inventory` - Shared inventory manager
    /// * `market` - Market contract account ID
    /// * `market_config` - Market configuration
    /// * `strategy` - Liquidation strategy
    /// * `collateral_strategy` - Collateral management strategy
    /// * `timeout` - Transaction timeout in seconds
    /// * `dry_run` - If true, scan and log without executing liquidations
    /// * `swap_provider` - Optional swap provider for collateral swaps
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: &JsonRpcClient,
        signer: Arc<Signer>,
        inventory: &inventory::SharedInventory,
        market: AccountId,
        market_config: MarketConfiguration,
        strategy: Arc<dyn LiquidationStrategy>,
        collateral_strategy: CollateralStrategy,
        timeout: u64,
        dry_run: bool,
        swap_provider: Option<crate::swap::SwapProviderImpl>,
    ) -> Self {
        let scanner = scanner::MarketScanner::new(client.clone(), market.clone());
        let oracle_fetcher = oracle::OracleFetcher::new(client.clone());
        let executor = executor::LiquidationExecutor::new(
            client.clone(),
            signer,
            inventory.clone(),
            market.clone(),
            collateral_strategy,
            timeout,
            dry_run,
            swap_provider,
        );

        Self {
            scanner,
            oracle_fetcher,
            executor,
            market,
            market_config,
            strategy,
        }
    }

    /// Get reference to the scanner (for compatibility checks)
    pub fn scanner(&self) -> &scanner::MarketScanner {
        &self.scanner
    }

    /// Performs a single liquidation using inventory-based model and modular architecture.
    ///
    /// # Flow
    /// 1. Scanner: Check if position is liquidatable
    /// 2. Strategy: Calculate liquidation amount
    /// 3. Estimate collateral value for profitability check
    /// 4. Profitability: Check if profitable
    /// 5. Executor: Execute liquidation (contract calculates optimal collateral to restore to MCR)
    #[tracing::instrument(skip(self, position, oracle_response), level = "info", fields(
        borrower = %borrow_account,
        market = %self.market
    ))]
    pub async fn liquidate(
        &self,
        borrow_account: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
    ) -> Result<LiquidationOutcome, LiquidatorError> {
        use templar_common::number::Decimal;

        // Step 1: Check liquidation status
        let status = self
            .scanner
            .get_borrow_status(&borrow_account, &oracle_response)
            .await
            .map_err(LiquidatorError::FetchBorrowStatus)?;

        let Some(BorrowStatus::Liquidation(reason)) = status else {
            debug!(
                borrower = %borrow_account,
                "Position is healthy, not liquidatable"
            );
            return Ok(LiquidationOutcome::NotLiquidatable);
        };

        if self.executor.is_dry_run() {
            info!(
                borrower = %borrow_account,
                reason = ?reason,
                collateral = %position.collateral_asset_deposit,
                "DRY RUN: Found liquidatable position"
            );
        } else {
            info!(
                borrower = %borrow_account,
                reason = ?reason,
                collateral = %position.collateral_asset_deposit,
                "Position is liquidatable"
            );
        }

        // Step 2: Calculate liquidation amount using strategy
        let available_balance = self
            .executor
            .inventory()
            .read()
            .await
            .get_available_balance(&self.market_config.borrow_asset);

        debug!(
            borrower = %borrow_account,
            available_balance = %available_balance.0,
            collateral_deposit = %position.collateral_asset_deposit,
            "Calculating liquidation amount"
        );

        let Some(liquidation_amount) = self.strategy.calculate_liquidation_amount(
            &position,
            &oracle_response,
            &self.market_config,
            available_balance,
        )?
        else {
            tracing::warn!(
                borrower = %borrow_account,
                available_balance = %available_balance.0,
                borrow_asset = %self.market_config.borrow_asset,
                collateral_deposit = %position.collateral_asset_deposit,
                "Cannot calculate liquidation amount (check: sufficient inventory, position viability, min 10% of full amount)"
            );
            return Ok(LiquidationOutcome::NotLiquidatable);
        };

        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            "Calculated liquidation amount"
        );

        // Step 3: Calculate collateral amount that corresponds to the liquidation amount
        // The strategy already calculated liquidation_amount as the minimum needed for target collateral
        // Simply calculate target collateral as percentage of total

        // Calculate target collateral as percentage of total
        let total_collateral = position.collateral_asset_deposit;
        let target_percentage_decimal =
            Decimal::from(u64::from(self.strategy.max_liquidation_percentage()))
                / Decimal::from(100u64);
        let target_collateral_decimal =
            Decimal::from(u128::from(total_collateral)) * target_percentage_decimal;
        let target_collateral_u128 = target_collateral_decimal.to_u128_floor().unwrap_or(0);

        // Use the target collateral, capped at available
        let collateral_amount = U128(target_collateral_u128.min(u128::from(total_collateral)));

        // Calculate expected value for profitability
        let expected_collateral_value =
            profitability::ProfitabilityCalculator::convert_collateral_to_borrow_asset(
                collateral_amount,
                &oracle_response,
                &self.market_config,
            )
            .unwrap_or(collateral_amount);

        debug!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            target_collateral = %collateral_amount.0,
            total_collateral = %u128::from(position.collateral_asset_deposit),
            estimated_collateral_value = %expected_collateral_value.0,
            target_percentage = %self.strategy.max_liquidation_percentage(),
            "Calculated target collateral for partial liquidation"
        );

        // Step 4: Check profitability

        let gas_cost = profitability::ProfitabilityCalculator::convert_gas_cost_to_borrow_asset(
            profitability::ProfitabilityCalculator::DEFAULT_GAS_COST_USD,
            &oracle_response,
            &self.market_config,
        )
        .unwrap_or(U128(50_000));

        // Calculate detailed profitability metrics
        let (net_profit, profit_pct) =
            profitability::ProfitabilityCalculator::calculate_profit_metrics(
                liquidation_amount,
                expected_collateral_value,
                gas_cost,
            );

        let is_profitable = self.strategy.should_liquidate(
            liquidation_amount,
            expected_collateral_value,
            gas_cost,
        )?;

        // Log detailed profitability analysis
        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            expected_collateral_value = %expected_collateral_value.0,
            gas_cost = %gas_cost.0,
            expected_revenue = %expected_collateral_value.0,
            net_profit = %net_profit,
            profit_percentage = %profit_pct,
            is_profitable = is_profitable,
            "Profitability analysis"
        );

        if !is_profitable {
            let prefix = if self.executor.is_dry_run() {
                "DRY RUN: "
            } else {
                ""
            };
            info!(
                borrower = %borrow_account,
                "{}Liquidation not profitable, skipping", prefix
            );
            return Ok(LiquidationOutcome::Unprofitable);
        }

        // Step 5: Execute liquidation (contract determines optimal collateral amount)
        self.executor
            .execute_liquidation(
                &borrow_account,
                &self.market_config.borrow_asset,
                &self.market_config.collateral_asset,
                liquidation_amount,
                collateral_amount,
                expected_collateral_value,
            )
            .await
    }

    /// Runs liquidations for all eligible positions in the market.
    #[tracing::instrument(skip(self, _concurrency), level = "info", fields(market = %self.market))]
    pub async fn run_liquidations(&self, _concurrency: usize) -> LiquidatorResult {
        let max_percentage = self.strategy.max_liquidation_percentage();

        info!(
            strategy = %self.strategy.strategy_name(),
            percentage = max_percentage,
            "Starting liquidation run"
        );

        // Fetch oracle prices
        let oracle_response = self
            .oracle_fetcher
            .get_oracle_prices(
                self.market_config
                    .price_oracle_configuration
                    .account_id
                    .clone(),
                &[
                    self.market_config
                        .price_oracle_configuration
                        .borrow_asset_price_id,
                    self.market_config
                        .price_oracle_configuration
                        .collateral_asset_price_id,
                ],
                self.market_config
                    .price_oracle_configuration
                    .price_maximum_age_s,
            )
            .await?;

        if oracle_response.is_empty() {
            return Ok(());
        }

        // Scan for positions
        let borrows = self.scanner.get_all_borrows().await?;
        if borrows.is_empty() {
            info!("No borrow positions found");
            return Ok(());
        }

        info!(positions = borrows.len(), "Evaluating positions");

        // Process positions
        let mut liquidated = 0;
        let mut not_liquidatable = 0;
        let mut unprofitable = 0;
        let mut failed = 0;
        let total = borrows.len();

        for (i, (account, position)) in borrows.into_iter().enumerate() {
            match self
                .liquidate(account.clone(), position, oracle_response.clone())
                .await
            {
                Ok(LiquidationOutcome::Liquidated) => liquidated += 1,
                Ok(LiquidationOutcome::NotLiquidatable) => not_liquidatable += 1,
                Ok(LiquidationOutcome::Unprofitable) => unprofitable += 1,
                Err(e) => {
                    tracing::warn!(borrower = %account, error = %e, "Liquidation failed");
                    failed += 1;
                }
            }

            if i < total - 1 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }

        info!(
            liquidated,
            not_liquidatable, unprofitable, failed, "Liquidation run completed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests;
