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
pub mod notifier;
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
    /// Position is no longer in a liquidatable state on-chain (became healthy
    /// or was liquidated by someone else). Distinct from `Skipped` —
    /// `Healthy` means the chain confirmed the position is OK.
    Healthy,
    /// We chose not to liquidate this round (insufficient inventory, below
    /// contract minimum, strategy returned no target, etc.). The position
    /// may still be liquidatable.
    Skipped,
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
    #[error("Failed to update on-chain oracle prices: {0}")]
    OracleUpdateError(String),
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

/// Classifies where in the liquidation pipeline an error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorPhase {
    /// Reading on-chain state, fetching prices, listing positions.
    Scan,
    /// Decided to liquidate but haven't submitted a tx yet (nonce, serialization, strategy).
    Preparation,
    /// Liquidation or swap transaction was submitted to the network.
    Execution,
}

impl ErrorPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Preparation => "preparation",
            Self::Execution => "execution",
        }
    }
}

impl std::fmt::Display for ErrorPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable, low-cardinality classification of failure kinds. Used as a dedup
/// bucket for repeat-failure notifications.
///
/// A typed enum (rather than a free-form string) prevents accidental
/// fragmentation of dedup state if a caller mistypes a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    ExcessiveLiquidation,
    OfferTooLow,
    NotEligible,
    ValueCalcFailure,
    TxTimeout,
    TxFailedOther,
    TxSubmissionError,
    SwapError,
    FetchBorrowStatus,
    PricePair,
    PriceFetch,
    ListPositions,
    ListDeployments,
    GetConfiguration,
    FetchBalance,
    AccessKey,
    Serialize,
    Strategy,
    InsufficientBalance,
    OracleUpdate,
}

impl NotificationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExcessiveLiquidation => "excessive_liquidation",
            Self::OfferTooLow => "offer_too_low",
            Self::NotEligible => "not_eligible",
            Self::ValueCalcFailure => "value_calc_failure",
            Self::TxTimeout => "tx_timeout",
            Self::TxFailedOther => "tx_failed_other",
            Self::TxSubmissionError => "tx_submission_error",
            Self::SwapError => "swap_error",
            Self::FetchBorrowStatus => "fetch_borrow_status",
            Self::PricePair => "price_pair",
            Self::PriceFetch => "price_fetch",
            Self::ListPositions => "list_positions",
            Self::ListDeployments => "list_deployments",
            Self::GetConfiguration => "get_configuration",
            Self::FetchBalance => "fetch_balance",
            Self::AccessKey => "access_key",
            Self::Serialize => "serialize",
            Self::Strategy => "strategy",
            Self::InsufficientBalance => "insufficient_balance",
            Self::OracleUpdate => "oracle_update",
        }
    }
}

impl std::fmt::Display for NotificationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl LiquidatorError {
    /// Classifies the error by pipeline phase.
    ///
    /// Only `Execution` errors trigger the "Liquidation Failed" Telegram
    /// notification (successful liquidations and swap issues have their own
    /// dedicated notifications sent elsewhere).
    /// `OracleUpdateError` is classified as `Preparation` because oracle price
    /// pushes are best-effort and swallowed before execution; they never
    /// propagate to callers in practice.
    pub const fn phase(&self) -> ErrorPhase {
        match self {
            Self::FetchBorrowStatus(_)
            | Self::PricePairError(_)
            | Self::PriceFetchError(_)
            | Self::ListBorrowPositionsError(_)
            | Self::ListDeploymentsError(_)
            | Self::GetConfigurationError(_)
            | Self::FetchBalanceError(_) => ErrorPhase::Scan,

            Self::AccessKeyDataError(_)
            | Self::SerializeError(_)
            | Self::StrategyError(_)
            | Self::InsufficientBalance
            | Self::OracleUpdateError(_) => ErrorPhase::Preparation,

            Self::LiquidationTransactionError(_)
            | Self::TransactionFailed(_)
            | Self::SwapProviderError(_) => ErrorPhase::Execution,
        }
    }

    /// Classifies the error into a stable dedup bucket for failure notifications.
    ///
    /// `TransactionFailed` is further classified by the contract panic
    /// substring so a "wrong amount" failure and an "offer too low" failure
    /// each fire their own notification once.
    #[must_use]
    pub fn notification_kind(&self) -> NotificationKind {
        match self {
            Self::TransactionFailed(msg) => classify_transaction_failure(msg),
            Self::LiquidationTransactionError(rpc::RpcError::TimeoutError(_, _)) => {
                NotificationKind::TxTimeout
            }
            Self::LiquidationTransactionError(_) => NotificationKind::TxSubmissionError,
            Self::SwapProviderError(_) => NotificationKind::SwapError,
            Self::FetchBorrowStatus(_) => NotificationKind::FetchBorrowStatus,
            Self::PricePairError(_) => NotificationKind::PricePair,
            Self::PriceFetchError(_) => NotificationKind::PriceFetch,
            Self::ListBorrowPositionsError(_) => NotificationKind::ListPositions,
            Self::ListDeploymentsError(_) => NotificationKind::ListDeployments,
            Self::GetConfigurationError(_) => NotificationKind::GetConfiguration,
            Self::FetchBalanceError(_) => NotificationKind::FetchBalance,
            Self::AccessKeyDataError(_) => NotificationKind::AccessKey,
            Self::SerializeError(_) => NotificationKind::Serialize,
            Self::StrategyError(_) => NotificationKind::Strategy,
            Self::InsufficientBalance => NotificationKind::InsufficientBalance,
            Self::OracleUpdateError(_) => NotificationKind::OracleUpdate,
        }
    }
}

/// Maps a contract-level `TransactionFailed` message to a stable kind.
///
/// The match is on substrings of the contract panic so the categorization
/// survives small wording changes and surrounding receipt-id boilerplate.
fn classify_transaction_failure(msg: &str) -> NotificationKind {
    if msg.contains("Attempt to liquidate more collateral") {
        NotificationKind::ExcessiveLiquidation
    } else if msg.contains("Liquidation offer too low") {
        NotificationKind::OfferTooLow
    } else if msg.contains("not eligible for liquidation") {
        NotificationKind::NotEligible
    } else if msg.contains("Failed to calculate value of collateral") {
        NotificationKind::ValueCalcFailure
    } else if msg.contains("Timeout") || msg.contains("timeout") {
        NotificationKind::TxTimeout
    } else {
        NotificationKind::TxFailedOther
    }
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
    /// Shared notifier for Telegram alerts
    notifier: crate::notifier::SharedNotifier,
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
        redstone_gateway_url: Option<String>,
        swap_retry_config: crate::swap::SwapRetryConfig,
        min_swap_value_usd: f64,
        proxy_oracle_cache: Option<oracle::ProxyOracleCache>,
        signer_for_oracle: Option<(AccountId, near_crypto::SecretKey)>,
        notifier: crate::notifier::SharedNotifier,
        nonce_tracker: crate::rpc::NonceTracker,
    ) -> Self {
        let scanner = scanner::MarketScanner::new(client.clone(), market.clone());
        let oracle_fetcher = oracle::OracleFetcher::new(
            client.clone(),
            hermes_url,
            redstone_gateway_url,
            proxy_oracle_cache,
            signer_for_oracle,
            nonce_tracker.clone(),
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
            market_config
                .price_oracle_configuration
                .collateral_asset_decimals,
            nonce_tracker,
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
            notifier,
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
    #[allow(clippy::too_many_lines)]
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
        let mut prices_pushed_onchain = false;
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

            let reason = match status {
                Some(BorrowStatus::Liquidation(r)) => r,
                Some(BorrowStatus::MaintenanceRequired) => {
                    // Position is no longer liquidatable but is still unhealthy
                    // — don't treat as Healthy (it would clear dedup state).
                    tracing::info!(
                        market = %self.market,
                        borrower = %borrow_account,
                        "Position no longer liquidatable but still requires maintenance, skipping"
                    );
                    return Ok(LiquidationOutcome::Skipped);
                }
                Some(BorrowStatus::Healthy) | None => {
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
                        LiquidationOutcome::Healthy
                    });
                }
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
                return Ok(LiquidationOutcome::Skipped);
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
                return Ok(LiquidationOutcome::Skipped);
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

            // Step 6: Push fresh prices to underlying Pyth oracle(s) before first execution.
            // The market contract reads from the on-chain oracle during liquidation,
            // so prices must be fresh there — not just in our HTTP-fetched view.
            // Resolves proxy/LST oracles to their underlying Pyth targets.
            // Only push once per liquidate() call (covers loop iterations too).
            if !prices_pushed_onchain && !dry_run {
                let oracle_account = &self.market_config.price_oracle_configuration.account_id;
                let price_ids = &[
                    self.market_config
                        .price_oracle_configuration
                        .borrow_asset_price_id,
                    self.market_config
                        .price_oracle_configuration
                        .collateral_asset_price_id,
                ];
                match self
                    .oracle_fetcher
                    .update_onchain_prices(oracle_account, price_ids)
                    .await
                {
                    Ok(_) => {
                        prices_pushed_onchain = true;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to update on-chain prices, proceeding with existing");
                        prices_pushed_onchain = true;
                    }
                }
            }

            // Execute liquidation (contract determines optimal collateral amount)
            let (outcome, swap_issue) = self
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

            // Notify about successful liquidation (sent first, before any swap issues)
            if outcome == LiquidationOutcome::Liquidated {
                let (borrow_dec, borrow_asset, coll_dec, coll_asset) = self.asset_info();
                let total_cost = liquidation_amount.0 + gas_cost.0;
                let signed_profit = if expected_collateral_value.0 >= total_cost {
                    i128::try_from(expected_collateral_value.0 - total_cost).unwrap_or(i128::MAX)
                } else {
                    -(i128::try_from(total_cost - expected_collateral_value.0).unwrap_or(i128::MAX))
                };
                self.notifier.notify_liquidation(
                    self.market.as_ref(),
                    borrow_account.as_ref(),
                    &format::format_amount_short(liquidation_amount.0, borrow_dec, &borrow_asset),
                    &format::format_amount_short(collateral_amount.0, coll_dec, &coll_asset),
                    &format::format_profit_short(
                        signed_profit,
                        liquidation_amount.0,
                        borrow_dec,
                        &borrow_asset,
                    ),
                    None,
                    dry_run,
                );
            }

            // Notify about swap issues (sent after liquidation notification)
            if let Some(issue) = swap_issue {
                match issue {
                    executor::SwapIssue::Unsupported { from, to, amount } => {
                        self.notifier.notify_swap_unsupported(
                            self.market.as_ref(),
                            &from,
                            &to,
                            &amount,
                        );
                    }
                    executor::SwapIssue::Failed {
                        from,
                        to,
                        amount,
                        error,
                    } => {
                        self.notifier.notify_swap_failed(
                            self.market.as_ref(),
                            &from,
                            &to,
                            &amount,
                            &error,
                        );
                    }
                }
            }

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
            match self.scanner.get_borrow_position(&borrow_account).await {
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

        // Fetch oracle prices via HTTP APIs (Hermes for Pyth, gateway for RedStone)
        let oracle_response = self
            .oracle_fetcher
            .get_oracle_prices(oracle_account.clone(), &price_ids, price_max_age)
            .await?;

        if oracle_response.is_empty() {
            tracing::warn!("Oracle returned no prices, skipping market");
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
                Ok(LiquidationOutcome::Liquidated) => {
                    self.notifier
                        .clear_failure_dedup_for(self.market.as_ref(), account.as_ref());
                    liquidated += 1;
                }
                Ok(LiquidationOutcome::Healthy) => {
                    self.notifier
                        .clear_failure_dedup_for(self.market.as_ref(), account.as_ref());
                    not_liquidatable += 1;
                }
                Ok(LiquidationOutcome::Skipped) => not_liquidatable += 1,
                Ok(LiquidationOutcome::Unprofitable) => unprofitable += 1,
                Err(e) => {
                    let phase = e.phase();
                    if phase == ErrorPhase::Execution {
                        tracing::error!(borrower = %account, phase = %phase, error = %e, "Liquidation failed");
                        self.notifier.notify_liquidation_failed(
                            self.market.as_ref(),
                            account.as_ref(),
                            e.notification_kind(),
                            &e.to_string(),
                        );
                    } else {
                        tracing::warn!(borrower = %account, phase = %phase, error = %e, "Skipped position");
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_phase_scan() {
        let err = LiquidatorError::FetchBorrowStatus(rpc::RpcError::TimeoutError(30, 30));
        assert_eq!(err.phase(), ErrorPhase::Scan);
    }

    #[test]
    fn test_error_phase_preparation() {
        let err = LiquidatorError::InsufficientBalance;
        assert_eq!(err.phase(), ErrorPhase::Preparation);
    }

    #[test]
    fn test_error_phase_execution() {
        let err = LiquidatorError::TransactionFailed("receipt failed".to_string());
        assert_eq!(err.phase(), ErrorPhase::Execution);
    }

    #[test]
    fn test_error_phase_display() {
        assert_eq!(ErrorPhase::Scan.to_string(), "scan");
        assert_eq!(ErrorPhase::Preparation.to_string(), "preparation");
        assert_eq!(ErrorPhase::Execution.to_string(), "execution");
    }

    #[test]
    fn test_notification_kind_excessive_liquidation() {
        let msg = r#"Receipt 6wy7eW4sLeVAApXmmsyaseK48yfGpJRVrt5etZrsRByp failed: ExecutionError("Smart contract panicked: Attempt to liquidate more collateral than is currently eligible: 37818981 requested > 34516659 available")"#;
        let err = LiquidatorError::TransactionFailed(msg.to_string());
        assert_eq!(
            err.notification_kind(),
            NotificationKind::ExcessiveLiquidation
        );
    }

    #[test]
    fn test_notification_kind_offer_too_low() {
        let err = LiquidatorError::TransactionFailed(
            "Smart contract panicked: Liquidation offer too low: 99 offered < 100".to_string(),
        );
        assert_eq!(err.notification_kind(), NotificationKind::OfferTooLow);
    }

    #[test]
    fn test_notification_kind_not_eligible() {
        let err = LiquidatorError::TransactionFailed(
            "Borrow position is not eligible for liquidation".to_string(),
        );
        assert_eq!(err.notification_kind(), NotificationKind::NotEligible);
    }

    #[test]
    fn test_notification_kind_value_calc_failure() {
        let err = LiquidatorError::TransactionFailed(
            "Smart contract panicked: Failed to calculate value of collateral".to_string(),
        );
        assert_eq!(err.notification_kind(), NotificationKind::ValueCalcFailure);
    }

    #[test]
    fn test_notification_kind_tx_failed_other() {
        let err = LiquidatorError::TransactionFailed("some new failure mode".to_string());
        assert_eq!(err.notification_kind(), NotificationKind::TxFailedOther);
    }

    #[test]
    fn test_notification_kind_tx_submission_timeout() {
        let err = LiquidatorError::LiquidationTransactionError(rpc::RpcError::TimeoutError(30, 30));
        assert_eq!(err.notification_kind(), NotificationKind::TxTimeout);
    }

    #[test]
    fn test_notification_kind_tx_submission_non_timeout() {
        let err = LiquidatorError::LiquidationTransactionError(rpc::RpcError::WrongResponseKind(
            "boom".to_string(),
        ));
        assert_eq!(err.notification_kind(), NotificationKind::TxSubmissionError);
    }

    #[test]
    fn test_notification_kind_non_tx_variants_stable() {
        assert_eq!(
            LiquidatorError::InsufficientBalance.notification_kind(),
            NotificationKind::InsufficientBalance,
        );
        assert_eq!(
            LiquidatorError::FetchBorrowStatus(rpc::RpcError::TimeoutError(30, 30))
                .notification_kind(),
            NotificationKind::FetchBorrowStatus,
        );
    }

    #[test]
    fn test_notification_kind_as_str_stable() {
        // Lock the string representation so dedup state from previous deployments
        // remains valid across rolling restarts.
        assert_eq!(
            NotificationKind::ExcessiveLiquidation.as_str(),
            "excessive_liquidation"
        );
        assert_eq!(NotificationKind::TxTimeout.as_str(), "tx_timeout");
        assert_eq!(
            NotificationKind::InsufficientBalance.as_str(),
            "insufficient_balance"
        );
    }
}
