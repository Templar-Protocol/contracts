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

use std::sync::Arc;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{json_types::U128, AccountId};
use templar_common::{
    borrow::{BorrowPosition, BorrowStatus},
    market::MarketConfiguration,
    oracle::pyth::OracleResponse,
};

use crate::liquidation_strategy::{LiquidationStrategy, SAFETY_BUFFER_BPS};

// Modules
pub mod config;
pub mod executor;
pub mod format;
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
    #[error("Failed to update Pyth prices: {0}")]
    PriceUpdateError(String),
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
    /// Market version (major, minor, patch) - used for version-specific liquidation logic
    market_version: Option<(u32, u32, u32)>,
    /// Enable automatic Pyth price updates before liquidations
    auto_update_prices: bool,
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
        hermes_url: Option<String>,
        auto_update_prices: bool,
        signer_for_oracle: &Option<(AccountId, near_crypto::SecretKey)>,
        swap_retry_config: crate::swap::SwapRetryConfig,
        min_swap_value_usd: f64,
    ) -> Self {
        let scanner = scanner::MarketScanner::new(client.clone(), market.clone());
        let oracle_fetcher = oracle::OracleFetcher::new(
            client.clone(),
            hermes_url,
            signer_for_oracle.as_ref().map(|(id, _)| id.clone()),
            signer_for_oracle.as_ref().map(|(_, key)| key.clone()),
        );
        let executor = executor::LiquidationExecutor::new(
            client.clone(),
            signer,
            inventory.clone(),
            market.clone(),
            timeout,
            dry_run,
            collateral_strategy,
            swap_provider,
            swap_retry_config,
            min_swap_value_usd,
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
            market_version: None,
            auto_update_prices,
        }
    }

    /// Get reference to the scanner (for compatibility checks)
    pub fn scanner(&self) -> &scanner::MarketScanner {
        &self.scanner
    }

    /// Get reference to the market configuration
    pub fn market_configuration(&self) -> &MarketConfiguration {
        &self.market_config
    }

    /// Fetches and caches the market version via NEP-330 contract metadata.
    ///
    /// This should be called once after creating the Liquidator to enable version-specific
    /// liquidation logic. The version determines whether to use total collateral (v1.0)
    /// or liquidatable collateral (v1.1+) for liquidation calculations.
    ///
    /// If the market doesn't provide NEP-330 metadata, assumes v1.0 for safety.
    pub async fn fetch_market_version(&mut self) {
        self.market_version = self.scanner.get_market_version().await;
        if let Some((major, minor, patch)) = self.market_version {
            tracing::debug!(
                market = %self.market,
                version = %format!("{major}.{minor}.{patch}"),
                "Fetched market version"
            );
        } else {
            tracing::debug!(
                market = %self.market,
                "Market version unavailable (no NEP-330 metadata), assuming v1.0"
            );
        }
    }

    /// Get formatted asset info for logging (decimals and asset IDs from configuration)
    fn asset_info(&self) -> (i32, String, i32, String) {
        let borrow_decimals = self
            .market_config
            .price_oracle_configuration
            .borrow_asset_decimals;
        let collateral_decimals = self
            .market_config
            .price_oracle_configuration
            .collateral_asset_decimals;
        let borrow_asset_id = self.market_config.borrow_asset.to_string();
        let collateral_asset_id = self.market_config.collateral_asset.to_string();
        (
            borrow_decimals,
            borrow_asset_id,
            collateral_decimals,
            collateral_asset_id,
        )
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
        // Loop liquidation support - controlled by LOOP_LIQUIDATION parameter
        // In dry run mode, skip looping since position state doesn't change
        // (no actual liquidation happens, so re-checking yields identical results)
        let dry_run = self.executor.is_dry_run();
        let loop_enabled = self.loop_liquidation && !dry_run;
        let mut loop_iteration = 0;
        let max_iterations = if dry_run { 1 } else { self.max_loop_iterations };
        let mut total_liquidated_amount = 0u128;
        let mut total_collateral_received = 0u128;
        let mut position = position;

        loop {
            loop_iteration += 1;

            if loop_enabled && loop_iteration > 1 {
                tracing::debug!(
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
                    let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();
                    tracing::info!(
                        market = %self.market,
                        borrower = %borrow_account,
                        iterations = loop_iteration - 1,
                        total_sent = %format::format_amount(total_liquidated_amount, borrow_dec, &borrow_asset),
                        total_received = %format::format_amount(total_collateral_received, coll_dec, &coll_asset),
                        "Loop liquidation completed successfully - position now healthy"
                    );
                }
                return Ok(if loop_iteration > 1 {
                    LiquidationOutcome::Liquidated
                } else {
                    LiquidationOutcome::NotLiquidatable
                });
            };

            // Log position is liquidatable with details
            if loop_iteration == 1 {
                let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();
                let price_pair = self
                    .market_config
                    .price_oracle_configuration
                    .create_price_pair(&oracle_response)?;
                let collateralization_ratio = position.collateralization_ratio(&price_pair);

                tracing::info!(
                    borrower = %borrow_account,
                    reason = ?reason,
                    mcr_liquidation = %self.market_config.borrow_mcr_liquidation,
                    collateralization_ratio = ?collateralization_ratio,
                    total_collateral = %format::format_amount(u128::from(position.collateral_asset_deposit), coll_dec, &coll_asset),
                    total_debt = %format::format_amount(u128::from(position.get_total_borrow_asset_liability()), borrow_dec, &borrow_asset),
                    "Position is liquidatable"
                );
            }

            // If loop liquidation is disabled or strategy doesn't support it, exit after first iteration
            if !loop_enabled && loop_iteration > 1 {
                tracing::info!(
                    borrower = %borrow_account,
                    "Loop liquidation not supported for this strategy, stopping after first liquidation"
                );
                return Ok(LiquidationOutcome::Liquidated);
            }

            // Safety check for max iterations
            if loop_iteration > max_iterations {
                let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();
                tracing::info!(
                    market = %self.market,
                    borrower = %borrow_account,
                    iterations = max_iterations,
                    total_sent = %format::format_amount(total_liquidated_amount, borrow_dec, &borrow_asset),
                    total_received = %format::format_amount(total_collateral_received, coll_dec, &coll_asset),
                    "Loop liquidation stopped - max iterations reached"
                );
                return Ok(LiquidationOutcome::Liquidated);
            }

            // Will log consolidated info after profitability check
            let dry_run_mode = self.executor.is_dry_run();

            // Step 2: Calculate liquidatable collateral
            // This amount determines the maximum collateral that can be liquidated
            // to bring the position to the maintenance collateralization ratio.
            let price_pair = self
                .market_config
                .price_oracle_configuration
                .create_price_pair(&oracle_response)?;
            let liquidatable_collateral = position.liquidatable_collateral(
                &price_pair,
                self.market_config.borrow_mcr_maintenance,
                self.market_config.liquidation_maximum_spread,
            );

            // Step 3: Calculate liquidation amount based on liquidatable collateral
            let available_balance = self
                .executor
                .inventory()
                .read()
                .await
                .get_available_balance(&self.market_config.borrow_asset);

            // Early check: ensure we have at least the contract minimum
            let contract_minimum: u128 = self.market_config.borrow_range.minimum.into();
            if available_balance.0 < contract_minimum {
                let (borrow_dec, borrow_asset, _, _) = self.asset_info();
                tracing::info!(
                    borrower = %borrow_account,
                    available_balance = %format::format_amount(available_balance.0, borrow_dec, &borrow_asset),
                    contract_minimum = %format::format_amount(contract_minimum, borrow_dec, &borrow_asset),
                    "Insufficient inventory: below contract minimum borrow amount, skipping"
                );
                return Ok(LiquidationOutcome::NotLiquidatable);
            }

            // v1.0.0 markets: use full position (no partial support)
            // v1.1.0+ markets: adjust position to liquidatable collateral for strategy calculation
            let adjusted_position = if self.market_version == Some((1, 0, 0)) {
                position.clone()
            } else {
                let mut adj = position.clone();
                adj.collateral_asset_deposit = liquidatable_collateral;
                adj
            };

            let (_, _, coll_dec, coll_asset) = self.asset_info();
            tracing::info!(
                borrower = %borrow_account,
                market = %self.market,
                market_version = ?self.market_version,
                liquidatable_collateral = %format::format_amount(liquidatable_collateral.into(), coll_dec, &coll_asset),
                total_collateral = %format::format_amount(position.collateral_asset_deposit.into(), coll_dec, &coll_asset),
                "Using liquidatable collateral for liquidation calculation"
            );

            let Some((liquidation_amount, collateral_amount)) =
                self.strategy.calculate_liquidation_amount(
                    &adjusted_position,
                    &oracle_response,
                    &self.market_config,
                    available_balance,
                    self.market_version,
                )?
            else {
                if loop_iteration > 1 {
                    let (borrow_dec, borrow_asset, _, _) = self.asset_info();
                    tracing::warn!(
                        borrower = %borrow_account,
                        iteration = %format::format_iteration(loop_iteration, max_iterations),
                        available_balance = %format::format_amount(available_balance.0, borrow_dec, &borrow_asset),
                        "Loop liquidation: insufficient balance to continue, stopping"
                    );
                    return Ok(LiquidationOutcome::Liquidated);
                }
                // Strategy already logged the specific reason (insufficient inventory, below minimum, etc.)
                return Ok(LiquidationOutcome::NotLiquidatable);
            };

            // Calculate expected value for profitability
            let expected_collateral_value =
                profitability::ProfitabilityCalculator::convert_collateral_to_borrow_asset(
                    collateral_amount,
                    &oracle_response,
                    &self.market_config,
                )
                .unwrap_or(collateral_amount);

            // Calculate what we'd actually get after applying liquidation spread
            // Spread reduces what we receive: value_after_spread = value × (1 - spread)
            let spread = self.market_config.liquidation_maximum_spread;
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let collateral_value_with_spread = {
                let spread_f64 = spread.to_f64_lossy();
                let value_f64 = expected_collateral_value.0 as f64;
                let after_spread = value_f64 * (1.0 - spread_f64);
                after_spread as u128
            };

            // Step 5: Check profitability

            let gas_cost =
                profitability::ProfitabilityCalculator::convert_gas_cost_to_borrow_asset(
                    profitability::ProfitabilityCalculator::DEFAULT_GAS_COST_USD,
                    &oracle_response,
                    &self.market_config,
                )
                .unwrap_or(U128(50_000));

            // Calculate detailed profitability metrics
            let (net_profit, _profit_pct) =
                profitability::ProfitabilityCalculator::calculate_profit_metrics(
                    liquidation_amount,
                    expected_collateral_value,
                    gas_cost,
                );

            let theoretical_amount_for_profit =
                U128((liquidation_amount.0 * 10_000) / (10_000 + SAFETY_BUFFER_BPS));

            let is_profitable = self.strategy.should_liquidate(
                theoretical_amount_for_profit,
                expected_collateral_value,
                gas_cost,
            )?;

            // Log consolidated liquidation info with human-readable amounts
            let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();

            if is_profitable {
                // Calculate signed profit for display (can be negative if unprofitable)
                let signed_profit =
                    if expected_collateral_value.0 >= liquidation_amount.0 + gas_cost.0 {
                        i128::try_from(net_profit).unwrap_or(i128::MAX)
                    } else {
                        // Revenue < cost, calculate actual loss
                        let loss = (liquidation_amount.0 + gas_cost.0)
                            .saturating_sub(expected_collateral_value.0);
                        -(i128::try_from(loss).unwrap_or(i128::MAX))
                    };

                let message = if dry_run_mode {
                    "[DRY RUN] Liquidatable position"
                } else {
                    "Liquidatable position"
                };

                // Only show iteration if loop is enabled (for partial/fixed strategies)
                if loop_enabled {
                    tracing::info!(
                        market = %self.market,
                        borrower = %borrow_account,
                        reason = ?reason,
                        iteration = %format::format_iteration(loop_iteration, max_iterations),
                        collateral_total = %format::format_amount(position.collateral_asset_deposit.into(), coll_dec, &coll_asset),
                        collateral_liquidatable = %format::format_amount(liquidatable_collateral.into(), coll_dec, &coll_asset),
                        send = %format::format_amount(liquidation_amount.0, borrow_dec, &borrow_asset),
                        receive = %format::format_amount(collateral_amount.0, coll_dec, &coll_asset),
                        profit = %format::format_profit(signed_profit, liquidation_amount.0, borrow_dec, &borrow_asset),
                        "{}", message
                    );
                } else {
                    tracing::info!(
                        market = %self.market,
                        borrower = %borrow_account,
                        reason = ?reason,
                        collateral_total = %format::format_amount(position.collateral_asset_deposit.into(), coll_dec, &coll_asset),
                        collateral_liquidatable = %format::format_amount(liquidatable_collateral.into(), coll_dec, &coll_asset),
                        send = %format::format_amount(liquidation_amount.0, borrow_dec, &borrow_asset),
                        receive = %format::format_amount(collateral_amount.0, coll_dec, &coll_asset),
                        profit = %format::format_profit(signed_profit, liquidation_amount.0, borrow_dec, &borrow_asset),
                        "{}", message
                    );
                }
            }

            if !is_profitable {
                let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();

                // Calculate actual loss (revenue - cost, will be negative)
                let total_cost = liquidation_amount.0 + gas_cost.0;
                let loss = if expected_collateral_value.0 >= total_cost {
                    i128::try_from(expected_collateral_value.0 - total_cost).unwrap_or(i128::MAX)
                } else {
                    let deficit = total_cost - expected_collateral_value.0;
                    -(i128::try_from(deficit).unwrap_or(i128::MAX))
                };

                // Calculate min required for profitability
                let profit_margin_multiplier = 10_000 + 50; // 50 bps default
                let min_revenue_required = (total_cost * profit_margin_multiplier) / 10_000;
                let spread_pct = spread.to_f64_lossy() * 100.0;

                let message = if dry_run_mode {
                    "[DRY RUN] Position not profitable, skipping"
                } else {
                    "Position not profitable, skipping"
                };

                tracing::info!(
                    market = %self.market,
                    borrower = %borrow_account,
                    collateral_total = %format::format_amount(position.collateral_asset_deposit.into(), coll_dec, &coll_asset),
                    collateral_liquidatable = %format::format_amount(liquidatable_collateral.into(), coll_dec, &coll_asset),
                    collateral_requested = %format::format_amount(collateral_amount.0, coll_dec, &coll_asset),
                    send = %format::format_amount(liquidation_amount.0, borrow_dec, &borrow_asset),
                    gas_cost = %format::format_amount(gas_cost.0, borrow_dec, &borrow_asset),
                    total_cost = %format::format_amount(total_cost, borrow_dec, &borrow_asset),
                    receive_value_no_spread = %format::format_amount(expected_collateral_value.0, borrow_dec, &borrow_asset),
                    receive_value_with_spread = %format::format_amount(collateral_value_with_spread, borrow_dec, &borrow_asset),
                    min_revenue_required = %format::format_amount(min_revenue_required, borrow_dec, &borrow_asset),
                    spread = %format!("{:.1}%", spread_pct),
                    loss = %format::format_profit(loss, total_cost, borrow_dec, &borrow_asset),
                    "{}", message
                );

                if loop_iteration > 1 {
                    return Ok(LiquidationOutcome::Liquidated);
                }
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

            tracing::debug!(
                borrower = %borrow_account,
                iteration = loop_iteration,
                liquidation_amount = %liquidation_amount.0,
                collateral_received = %collateral_amount.0,
                cumulative_liquidated = total_liquidated_amount,
                cumulative_collateral = total_collateral_received,
                "Liquidation iteration completed"
            );

            // If loop liquidation is disabled or strategy doesn't support it, return after first liquidation
            if !loop_enabled {
                return Ok(outcome);
            }

            // Re-fetch position data before next iteration so we have current
            // collateral/debt amounts (the status check at the top of the loop
            // only checks liquidation eligibility, not position amounts).
            match self
                .scanner
                .get_borrow_position(&borrow_account)
                .await
            {
                Ok(Some(updated)) => position = updated,
                Ok(None) => {
                    tracing::info!(
                        borrower = %borrow_account,
                        "Position no longer exists after liquidation, stopping loop"
                    );
                    return Ok(LiquidationOutcome::Liquidated);
                }
                Err(e) => {
                    tracing::warn!(
                        borrower = %borrow_account,
                        error = ?e,
                        "Failed to re-fetch position, stopping loop"
                    );
                    return Ok(LiquidationOutcome::Liquidated);
                }
            }
        }
    }

    /// Runs liquidations for all eligible positions in the market.
    #[tracing::instrument(skip(self, _concurrency), level = "info", fields(market = %self.market))]
    pub async fn run_liquidations(&self, _concurrency: usize) -> LiquidatorResult {
        let max_percentage = self.strategy.max_liquidation_percentage();

        tracing::info!(
            strategy = %self.strategy.strategy_name(),
            percentage = max_percentage,
            auto_update_prices = self.auto_update_prices,
            "Starting liquidation run"
        );

        let oracle_account = self
            .market_config
            .price_oracle_configuration
            .account_id
            .clone();
        let price_ids = [
            self.market_config
                .price_oracle_configuration
                .borrow_asset_price_id,
            self.market_config
                .price_oracle_configuration
                .collateral_asset_price_id,
        ];
        let price_max_age = self
            .market_config
            .price_oracle_configuration
            .price_maximum_age_s;

        // Fetch oracle prices
        let mut oracle_response = self
            .oracle_fetcher
            .get_oracle_prices(oracle_account.clone(), &price_ids, price_max_age)
            .await?;

        // Check if any prices are missing or stale
        let now = if let Ok(duration) =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        {
            duration.as_secs().try_into().unwrap_or(i64::MAX)
        } else {
            tracing::error!("System time is before UNIX epoch");
            return Err(LiquidatorError::PriceUpdateError(
                "System time error".to_string(),
            ));
        };

        let has_stale_prices = oracle_response.is_empty()
            || price_ids.iter().any(|price_id| {
                oracle_response
                    .get(price_id)
                    .and_then(|opt| opt.as_ref())
                    .is_none_or(|price| (now - price.publish_time) > i64::from(price_max_age))
            });

        // If prices are missing/stale and auto-update is enabled, try to update them
        let dry_run = self.executor.is_dry_run();
        if has_stale_prices && self.auto_update_prices {
            if dry_run {
                tracing::info!(
                    price_ids = ?price_ids,
                    max_age_s = price_max_age,
                    "[DRY RUN] Oracle prices are missing or stale, skipping on-chain update"
                );
            } else {
                tracing::warn!(
                    price_ids = ?price_ids,
                    max_age_s = price_max_age,
                    "Oracle prices are missing or stale, attempting to update from Pyth Hermes (AUTO_UPDATE_PRICES=true)"
                );

                match self
                    .oracle_fetcher
                    .update_pyth_prices(&oracle_account, &price_ids)
                    .await
                {
                    Ok(true) => {
                        tracing::info!("Successfully updated Pyth prices, re-fetching");
                        oracle_response = self
                            .oracle_fetcher
                            .get_oracle_prices(oracle_account.clone(), &price_ids, price_max_age)
                            .await?;
                    }
                    Ok(false) => {
                        tracing::warn!("Price update was skipped (no signer or already fresh)");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to update Pyth prices");
                    }
                }
            }
        } else if has_stale_prices {
            tracing::warn!(
                auto_update_prices = self.auto_update_prices,
                price_ids = ?price_ids,
                max_age_s = price_max_age,
                "Oracle prices are missing or stale. Enable AUTO_UPDATE_PRICES=true to automatically update prices before liquidations."
            );
        }

        if oracle_response.is_empty() {
            return Ok(());
        }

        // Scan for positions
        let borrows = self.scanner.get_all_borrows().await?;
        if borrows.is_empty() {
            tracing::info!("No borrow positions found");
            return Ok(());
        }

        tracing::info!(positions = borrows.len(), "Evaluating positions");

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

        tracing::info!(
            liquidated,
            not_liquidatable,
            unprofitable,
            failed,
            "Liquidation run completed"
        );

        Ok(())
    }
}
