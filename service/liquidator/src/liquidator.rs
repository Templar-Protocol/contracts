//! Liquidator bot with modular architecture.
//!
//! Provides inventory-based liquidation with:
//! - Modular component architecture
//! - Pluggable liquidation strategies
//! - Error handling
//! - Gas cost estimation and profitability analysis
//!
//! Components:
//! - `service`: Bot lifecycle management
//! - `scanner`: Market position scanning
//! - `executor`: Transaction execution
//! - `oracle`: Price fetching
//! - `profitability`: Cost/profit calculations
//! - `inventory`: Asset balance tracking
//! - `strategy`: Liquidation amount calculations
//! - `swap`: Swap provider implementations
//!
//!   service.run().await;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{json_types::U128, AccountId};
use templar_common::{
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
    /// Enable loop liquidation - repeatedly liquidate until position is healthy
    loop_liquidation: bool,
    /// Maximum iterations for loop liquidation (safety limit)
    max_loop_iterations: u32,
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
    /// * `loop_liquidation` - Enable loop liquidation until position is healthy
    /// * `max_loop_iterations` - Maximum iterations for loop liquidation (safety limit)
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
        loop_liquidation: bool,
        max_loop_iterations: u32,
    ) -> Self {
        let scanner = scanner::MarketScanner::new(client.clone(), market.clone());
        let oracle_fetcher = oracle::OracleFetcher::new(client.clone());
        let executor = executor::LiquidationExecutor::new(
            client.clone(),
            signer,
            inventory.clone(),
            market.clone(),
            timeout,
            dry_run,
            collateral_strategy,
            swap_provider,
        );

        Self {
            scanner,
            oracle_fetcher,
            executor,
            market,
            market_config,
            strategy,
            loop_liquidation,
            max_loop_iterations,
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
        // Loop liquidation support
        let mut loop_iteration = 0;
        let max_iterations = self.max_loop_iterations;
        let mut total_liquidated_amount = 0u128;
        let mut total_collateral_received = 0u128;

        loop {
            loop_iteration += 1;

            if self.loop_liquidation && loop_iteration > 1 {
                info!(
                    borrower = %borrow_account,
                    iteration = loop_iteration,
                    total_liquidated = total_liquidated_amount,
                    total_collateral = total_collateral_received,
                    "Loop liquidation: checking position again"
                );
            }

            // Step 1: Check liquidation status
            let status = self
                .scanner
                .get_borrow_status(&borrow_account, &oracle_response)
                .await
                .map_err(LiquidatorError::FetchBorrowStatus)?;

            let Some(BorrowStatus::Liquidation(reason)) = status else {
                if loop_iteration > 1 {
                    info!(
                        borrower = %borrow_account,
                        iterations = loop_iteration - 1,
                        total_liquidated = total_liquidated_amount,
                        total_collateral = total_collateral_received,
                        "Loop liquidation: position is now healthy"
                    );
                } else {
                    debug!(
                        borrower = %borrow_account,
                        "Position is healthy, not liquidatable"
                    );
                }
                return Ok(if loop_iteration > 1 {
                    LiquidationOutcome::Liquidated
                } else {
                    LiquidationOutcome::NotLiquidatable
                });
            };

            // If loop liquidation is disabled and we've already done one iteration, exit
            if !self.loop_liquidation && loop_iteration > 1 {
                info!(
                    borrower = %borrow_account,
                    "Loop liquidation disabled, stopping after first liquidation"
                );
                return Ok(LiquidationOutcome::Liquidated);
            }

            // Safety check for max iterations
            if loop_iteration > max_iterations {
                tracing::info!(
                    borrower = %borrow_account,
                    max_iterations,
                    "Reached maximum loop iterations, stopping"
                );
                return Ok(LiquidationOutcome::Liquidated);
            }

            if self.executor.is_dry_run() {
                info!(
                    borrower = %borrow_account,
                    iteration = loop_iteration,
                    reason = ?reason,
                    collateral = %position.collateral_asset_deposit,
                    "DRY RUN: Found liquidatable position"
                );
            } else {
                info!(
                    borrower = %borrow_account,
                    iteration = loop_iteration,
                    reason = ?reason,
                    collateral = %position.collateral_asset_deposit,
                    "Position is liquidatable"
                );
            }

            // Step 2: Calculate liquidatable collateral first
            // We need to know the actual liquidatable amount before calculating liquidation_amount
            let price_pair = self
                .market_config
                .price_oracle_configuration
                .create_price_pair(&oracle_response)?;
            let liquidatable_collateral = position.liquidatable_collateral(
                &price_pair,
                self.market_config.borrow_mcr_liquidation,
                self.market_config.liquidation_maximum_spread,
            );

            debug!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                liquidatable_collateral = %u128::from(liquidatable_collateral),
                total_collateral = %u128::from(position.collateral_asset_deposit),
                "Calculated liquidatable collateral"
            );

            // Step 3: Calculate liquidation amount based on liquidatable collateral
            let available_balance = self
                .executor
                .inventory()
                .read()
                .await
                .get_available_balance(&self.market_config.borrow_asset);

            debug!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                available_balance = %available_balance.0,
                collateral_deposit = %position.collateral_asset_deposit,
                "Calculating liquidation amount"
            );

            // Create a temporary position with liquidatable collateral for strategy calculation
            let mut adjusted_position = position.clone();
            adjusted_position.collateral_asset_deposit = liquidatable_collateral;

            let Some((liquidation_amount, collateral_amount)) =
                self.strategy.calculate_liquidation_amount(
                    &adjusted_position,
                    &oracle_response,
                    &self.market_config,
                    available_balance,
                )?
            else {
                if loop_iteration > 1 {
                    tracing::warn!(
                        borrower = %borrow_account,
                        iteration = loop_iteration,
                        available_balance = %available_balance.0,
                        "Loop liquidation: insufficient balance to continue, stopping"
                    );
                    return Ok(LiquidationOutcome::Liquidated);
                }
                tracing::warn!(
                    borrower = %borrow_account,
                    available_balance = %available_balance.0,
                    borrow_asset = %self.market_config.borrow_asset,
                    liquidatable_collateral = %u128::from(liquidatable_collateral),
                    total_collateral = %u128::from(position.collateral_asset_deposit),
                    "Cannot calculate liquidation amount (check: sufficient inventory, position viability, minimum value threshold)"
                );
                return Ok(LiquidationOutcome::NotLiquidatable);
            };

            info!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                liquidation_amount = %liquidation_amount.0,
                collateral_amount = %collateral_amount.0,
                "Calculated liquidation and collateral amounts from strategy"
            );

            // Calculate expected value for profitability
            let expected_collateral_value =
                profitability::ProfitabilityCalculator::convert_collateral_to_borrow_asset(
                    collateral_amount,
                    &oracle_response,
                    &self.market_config,
                )
                .unwrap_or(collateral_amount);

            info!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                liquidation_amount = %liquidation_amount.0,
                collateral_amount = %collateral_amount.0,
                liquidatable_collateral = %u128::from(liquidatable_collateral),
                total_collateral = %u128::from(position.collateral_asset_deposit),
                estimated_collateral_value = %expected_collateral_value.0,
                target_percentage = %self.strategy.max_liquidation_percentage(),
                "Calculated target collateral based on liquidatable amount"
            );

            // Step 5: Check profitability

            let gas_cost =
                profitability::ProfitabilityCalculator::convert_gas_cost_to_borrow_asset(
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
                iteration = loop_iteration,
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
                if loop_iteration > 1 {
                    info!(
                        borrower = %borrow_account,
                        iteration = loop_iteration,
                        "{}Loop liquidation: not profitable to continue, stopping", prefix
                    );
                    return Ok(LiquidationOutcome::Liquidated);
                }
                info!(
                    borrower = %borrow_account,
                    "{}Liquidation not profitable, skipping", prefix
                );
                return Ok(LiquidationOutcome::Unprofitable);
            }

            // Step 6: Execute liquidation (contract determines optimal collateral amount)
            let outcome = self
                .executor
                .execute_liquidation(
                    &borrow_account,
                    &self.market_config.borrow_asset,
                    &self.market_config.collateral_asset,
                    templar_common::asset::BorrowAssetAmount::from(liquidation_amount.0),
                    templar_common::asset::CollateralAssetAmount::from(collateral_amount.0),
                    templar_common::asset::BorrowAssetAmount::from(expected_collateral_value.0),
                )
                .await?;

            // Track cumulative amounts
            total_liquidated_amount += liquidation_amount.0;
            total_collateral_received += collateral_amount.0;

            info!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                liquidation_amount = %liquidation_amount.0,
                collateral_received = %collateral_amount.0,
                cumulative_liquidated = total_liquidated_amount,
                cumulative_collateral = total_collateral_received,
                "Liquidation iteration completed"
            );

            // If loop liquidation is disabled, return immediately after first liquidation
            if !self.loop_liquidation {
                return Ok(outcome);
            }

            // If we get here and loop_liquidation is enabled, continue to next iteration
            // The loop will re-check the position status at the top
        }
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
