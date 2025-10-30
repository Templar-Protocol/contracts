// SPDX-License-Identifier: MIT
//! Production-grade liquidator bot with extensible architecture.
//!
//! This module provides a modern liquidator implementation with:
//! - Strategy pattern for flexible liquidation approaches
//! - Pluggable swap providers (Rhea, NEAR Intents, etc.)
//! - Comprehensive error handling and logging
//! - Gas cost estimation
//! - Profitability analysis
//!
//! # Example
//!
//! ```no_run
//! use templar_bots::liquidator::Liquidator;
//! use templar_bots::strategy::PartialLiquidationStrategy;
//! use templar_bots::swap::{SwapProvider, rhea::RheaSwap};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let strategy = PartialLiquidationStrategy::default_partial();
//! let swap_provider = RheaSwap::new(contract, client.clone(), signer.clone());
//!
//! let liquidator = Liquidator::new(
//!     client,
//!     signer,
//!     asset,
//!     market,
//!     swap_provider,
//!     Box::new(strategy),
//!     timeout,
//! );
//!
//! liquidator.run_liquidations(10).await?;
//! # Ok(())
//! # }
//! ```

use std::{collections::HashMap, sync::Arc, str::FromStr};

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;

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
use near_primitives::{
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId,
};
use templar_common::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    market::{DepositMsg, LiquidateMsg, MarketConfiguration},
    number::Decimal,
    oracle::{
        price_transformer::PriceTransformer,
        pyth::{OracleResponse, PriceIdentifier},
    },
};
use tracing::{debug, error, info, warn, Span};

use crate::{
    rpc::{get_access_key_data, send_tx, view, AppError, BorrowPositions, RpcError},
    strategy::LiquidationStrategy,
    swap::{SwapProvider, SwapProviderImpl},
};

pub mod rpc;
pub mod strategy;
pub mod swap;

// Implement From for AppError to LiquidatorError
impl From<AppError> for LiquidatorError {
    fn from(err: AppError) -> Self {
        LiquidatorError::SwapProviderError(err)
    }
}

/// Errors that can occur during liquidation operations.
#[derive(Debug, thiserror::Error)]
pub enum LiquidatorError {
    #[error("Failed to fetch borrow status: {0}")]
    FetchBorrowStatus(RpcError),
    #[error("Failed to serialize data: {0}")]
    SerializeError(#[from] serde_json::Error),
    #[error("Price pair retrieval error: {0}")]
    PricePairError(#[from] templar_common::market::error::RetrievalError),
    #[error("Swap provider error: {0}")]
    SwapProviderError(AppError),
    #[error("Failed to get market configuration: {0}")]
    GetConfigurationError(RpcError),
    #[error("Failed to fetch oracle prices: {0}")]
    PriceFetchError(RpcError),
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(RpcError),
    #[error("Liquidation transaction error: {0}")]
    LiquidationTransactionError(RpcError),
    #[error("Failed to list borrow positions: {0}")]
    ListBorrowPositionsError(RpcError),
    #[error("Failed to fetch balance: {0}")]
    FetchBalanceError(RpcError),
    #[error("Failed to list deployments: {0}")]
    ListDeploymentsError(RpcError),
    #[error("Strategy error: {0}")]
    StrategyError(String),
    #[error("Insufficient balance for liquidation")]
    InsufficientBalance,
}

pub type LiquidatorResult<T = ()> = Result<T, LiquidatorError>;

/// Production-grade liquidator with extensible architecture.
///
/// This liquidator supports:
/// - Multiple swap providers (Rhea, NEAR Intents, custom implementations)
/// - Configurable liquidation strategies (partial, full, custom)
/// - Comprehensive logging and monitoring
/// - Gas cost optimization
/// - Profitability analysis
pub struct Liquidator {
    /// JSON-RPC client for blockchain interaction
    client: JsonRpcClient,
    /// Transaction signer
    signer: Arc<Signer>,
    /// Asset to use for liquidations
    asset: Arc<FungibleAsset<BorrowAsset>>,
    /// Market contract to liquidate positions in
    pub market: AccountId,
    /// Swap provider for asset exchanges
    swap_provider: SwapProviderImpl,
    /// Liquidation strategy
    strategy: Box<dyn LiquidationStrategy>,
    /// Transaction timeout in seconds
    timeout: u64,
    /// Dry run mode - scan and log without executing liquidations
    dry_run: bool,
}

impl Liquidator {
    /// Minimum supported contract version (semver).
    /// Markets with version < 1.0.0 will be skipped.
    const MIN_SUPPORTED_VERSION: (u32, u32, u32) = (1, 0, 0);

    /// Creates a new liquidator instance.
    ///
    /// # Arguments
    ///
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    /// * `asset` - Asset to use for liquidations
    /// * `market` - Market contract account ID
    /// * `swap_provider` - Swap provider implementation
    /// * `strategy` - Liquidation strategy
    /// * `timeout` - Transaction timeout in seconds
    /// * `dry_run` - If true, scan and log without executing liquidations
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        asset: Arc<FungibleAsset<BorrowAsset>>,
        market: AccountId,
        swap_provider: SwapProviderImpl,
        strategy: Box<dyn LiquidationStrategy>,
        timeout: u64,
        dry_run: bool,
    ) -> Self {
        Self {
            client,
            signer,
            asset,
            market,
            swap_provider,
            strategy,
            timeout,
            dry_run,
        }
    }

    /// Default gas cost estimate in USD
    /// ~$0.05 USD for a liquidation transaction (conservative estimate for 0.01 NEAR at ~$5)
    /// This will be converted to borrow asset units based on oracle prices
    const DEFAULT_GAS_COST_USD: f64 = 0.05;

    /// Tests if the market is compatible and can be monitored.
    /// This method is public for use during startup market filtering.
    pub async fn test_market_compatibility(&self) -> LiquidatorResult<()> {
        let is_compatible = self.is_market_compatible().await?;
        if !is_compatible {
            return Err(LiquidatorError::StrategyError(
                "Market version is not supported".to_string(),
            ));
        }
        Ok(())
    }

    /// Checks if the market contract is compatible by verifying its version via NEP-330.
    /// Returns true if version >= 1.0.0, false otherwise.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn is_market_compatible(&self) -> LiquidatorResult<bool> {
        use crate::rpc::get_contract_version;

        let version_string = match get_contract_version(&self.client, &self.market).await {
            Some(v) => v,
            None => {
                info!(
                    market = %self.market,
                    "Contract does not implement NEP-330 (contract_source_metadata), assuming compatible"
                );
                return Ok(true);
            }
        };

        // Parse semver (e.g., "1.2.3" or "0.1.0")
        let parts: Vec<&str> = version_string.split('.').collect();
        let (major, minor, patch) = match parts.as_slice() {
            [maj, min, pat] => {
                let major = maj.parse::<u32>().unwrap_or(0);
                let minor = min.parse::<u32>().unwrap_or(0);
                let patch = pat.parse::<u32>().unwrap_or(0);
                (major, minor, patch)
            }
            _ => {
                warn!(
                    market = %self.market,
                    version = %version_string,
                    "Invalid semver format, assuming compatible"
                );
                return Ok(true);
            }
        };

        let is_compatible = (major, minor, patch) >= Self::MIN_SUPPORTED_VERSION;

        if !is_compatible {
            info!(
                market = %self.market,
                version = %version_string,
                min_version = "1.0.0",
                "Skipping market - unsupported contract version"
            );
        }

        Ok(is_compatible)
    }

    /// Fetches the market configuration.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_configuration(&self) -> LiquidatorResult<MarketConfiguration> {
        view(
            &self.client,
            self.market.clone(),
            "get_configuration",
            json!({}),
        )
        .await
        .map_err(LiquidatorError::GetConfigurationError)
    }

    /// Fetches current oracle prices.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        // Try `list_ema_prices_unsafe` first (Pyth oracle)
        // The "unsafe" variant returns potentially stale prices without trying to update them,
        // which is acceptable for liquidation bots as we validate profitability before executing.
        let result: Result<OracleResponse, _> = view(
            &self.client,
            oracle.clone(),
            "list_ema_prices_unsafe",
            json!({ "price_ids": price_ids }),
        )
        .await;

        match result {
            Ok(response) => Ok(response),
            Err(e) => {
                // Use Debug format to get full error details including ProhibitedInView
                let error_msg = format!("{:?}", e);
                tracing::debug!("First oracle call failed for {}: {}", oracle, error_msg);

                // Check if oracle creates promises in view calls (incompatible with liquidation bot)
                if error_msg.contains("ProhibitedInView") {
                    tracing::debug!(
                        oracle = %oracle,
                        "Oracle creates promises in view calls, trying LST oracle approach"
                    );
                    return self.get_oracle_prices_with_transformers(oracle, price_ids, age).await;
                }

                // If method not found, try the standard method with age validation
                if error_msg.contains("MethodNotFound") || error_msg.contains("MethodResolveError")
                {
                    tracing::debug!(
                        "Oracle {} doesn't support list_ema_prices_unsafe, trying list_ema_prices_no_older_than",
                        oracle
                    );

                    match view(
                        &self.client,
                        oracle.clone(),
                        "list_ema_prices_no_older_than",
                        json!({ "price_ids": price_ids, "age": age }),
                    )
                    .await
                    {
                        Ok(response) => {
                            tracing::info!(
                                "Successfully fetched prices from {} using list_ema_prices_no_older_than",
                                oracle
                            );
                            Ok(response)
                        }
                        Err(fallback_err) => {
                            // Use Debug format to get full error details
                            let fallback_error_msg = format!("{:?}", fallback_err);

                            // Check if fallback also fails with ProhibitedInView
                            if fallback_error_msg.contains("ProhibitedInView") {
                                tracing::debug!(
                                    oracle = %oracle,
                                    "Fallback also creates promises, trying LST oracle approach"
                                );
                                return self.get_oracle_prices_with_transformers(oracle, price_ids, age).await;
                            }
                            Err(LiquidatorError::PriceFetchError(fallback_err))
                        }
                    }
                } else {
                    Err(LiquidatorError::PriceFetchError(e))
                }
            }
        }
    }

    /// Fetches prices from LST oracle by calling underlying Pyth oracle and applying transformers.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_oracle_prices_with_transformers(
        &self,
        lst_oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        tracing::info!(
            oracle = %lst_oracle,
            "Detected LST oracle, fetching transformers and applying manually"
        );

        // Get transformers for each price ID
        let mut transformers: HashMap<PriceIdentifier, PriceTransformer> = HashMap::new();
        let mut underlying_price_ids: Vec<PriceIdentifier> = Vec::new();

        for &price_id in price_ids {
            match view::<Option<PriceTransformer>>(
                &self.client,
                lst_oracle.clone(),
                "get_transformer",
                json!({ "price_identifier": price_id }),
            )
            .await
            {
                Ok(Some(transformer)) => {
                    tracing::debug!(
                        price_id = ?price_id,
                        underlying_id = ?transformer.price_id,
                        "Found price transformer"
                    );
                    underlying_price_ids.push(transformer.price_id);
                    transformers.insert(price_id, transformer);
                }
                Ok(None) => {
                    tracing::debug!(price_id = ?price_id, "No transformer, using price ID as-is");
                    underlying_price_ids.push(price_id);
                }
                Err(e) => {
                    tracing::warn!(
                        price_id = ?price_id,
                        error = %e,
                        "Failed to get transformer, skipping market"
                    );
                    return Ok(HashMap::new());
                }
            }
        }

        // Get underlying oracle account ID
        let underlying_oracle: AccountId = match view(
            &self.client,
            lst_oracle.clone(),
            "oracle_id",
            json!({}),
        )
        .await
        {
            Ok(oracle_id) => oracle_id,
            Err(e) => {
                tracing::warn!(
                    oracle = %lst_oracle,
                    error = %e,
                    "Failed to get underlying oracle ID, skipping market"
                );
                return Ok(HashMap::new());
            }
        };

        tracing::debug!(
            underlying_oracle = %underlying_oracle,
            underlying_price_ids = ?underlying_price_ids,
            "Fetching prices from underlying Pyth oracle"
        );

        // Fetch prices from underlying Pyth oracle (use Box::pin to avoid infinite recursion)
        let mut underlying_prices = Box::pin(self
            .get_oracle_prices(underlying_oracle.clone(), &underlying_price_ids, age))
            .await?;

        if underlying_prices.is_empty() {
            tracing::warn!("Underlying oracle returned no prices, skipping market");
            return Ok(HashMap::new());
        }

        // Apply transformers to get final prices
        let mut final_prices: OracleResponse = HashMap::new();

        for (&original_price_id, transformer) in &transformers {
            if let Some(Some(underlying_price)) = underlying_prices.remove(&transformer.price_id) {
                // Need to get the input value for transformation (e.g., LST redemption rate)
                match self
                    .fetch_transformer_input(&transformer.call, &lst_oracle)
                    .await
                {
                    Ok(input) => {
                        if let Some(transformed_price) =
                            transformer.action.apply(underlying_price, input)
                        {
                            tracing::debug!(
                                price_id = ?original_price_id,
                                "Successfully transformed price"
                            );
                            final_prices.insert(original_price_id, Some(transformed_price));
                        } else {
                            tracing::warn!(
                                price_id = ?original_price_id,
                                "Price transformation returned None"
                            );
                            final_prices.insert(original_price_id, None);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            price_id = ?original_price_id,
                            error = %e,
                            "Failed to fetch transformer input"
                        );
                        final_prices.insert(original_price_id, None);
                    }
                }
            } else {
                tracing::warn!(
                    price_id = ?original_price_id,
                    underlying_id = ?transformer.price_id,
                    "Underlying price not found in oracle response"
                );
                final_prices.insert(original_price_id, None);
            }
        }

        // Add prices that didn't need transformation
        for &price_id in price_ids {
            if !transformers.contains_key(&price_id) {
                if let Some(price) = underlying_prices.remove(&price_id) {
                    final_prices.insert(price_id, price);
                }
            }
        }

        tracing::info!(
            oracle = %lst_oracle,
            price_count = final_prices.len(),
            "Successfully fetched and transformed LST oracle prices"
        );

        Ok(final_prices)
    }

    /// Fetches the input value needed for price transformation (e.g., LST redemption rate).
    async fn fetch_transformer_input(
        &self,
        call: &templar_common::oracle::price_transformer::Call,
        _oracle: &AccountId,
    ) -> Result<Decimal, RpcError> {
        // Use the rpc_call() method to create a view query
        let query = call.rpc_call();

        // Execute the query using the RPC client
        let request = near_jsonrpc_client::methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: query,
        };

        let response = self.client.call(request).await.map_err(RpcError::from)?;

        // Parse the result
        if let near_jsonrpc_primitives::types::query::QueryResponseKind::CallResult(result) =
            response.kind
        {
            let value: Decimal =
                serde_json::from_slice(&result.result).map_err(RpcError::DeserializeError)?;
            Ok(value)
        } else {
            Err(RpcError::WrongResponseKind(
                "Expected CallResult".to_string(),
            ))
        }
    }

    /// Fetches borrow status for an account.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: &OracleResponse,
    ) -> Result<Option<BorrowStatus>, RpcError> {
        view(
            &self.client,
            self.market.clone(),
            "get_borrow_status",
            &json!({
                "account_id": account_id,
                "oracle_response": oracle_response,
            }),
        )
        .await
    }

    /// Fetches all borrow positions from the market.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> LiquidatorResult<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let page: BorrowPositions = view(
                &self.client,
                self.market.clone(),
                "list_borrow_positions",
                json!({
                    "offset": current_offset,
                    "count": page_size,
                }),
            )
            .await
            .map_err(LiquidatorError::ListBorrowPositionsError)?;

            let fetched = page.len();
            if fetched == 0 {
                break;
            }

            all_positions.extend(page);
            current_offset += fetched;

            if fetched < page_size {
                break;
            }
        }

        Ok(all_positions)
    }

    /// Converts USD gas cost estimate to borrow asset units using oracle prices.
    ///
    /// Formula: gas_cost_borrow_asset = gas_cost_usd / borrow_asset_usd_price * 10^borrow_decimals
    ///
    /// # Arguments
    ///
    /// * `gas_cost_usd` - Gas cost in USD (e.g., 0.05 for $0.05)
    /// * `oracle_response` - Oracle price data containing borrow asset/USD price
    /// * `configuration` - Market configuration containing borrow asset price ID and decimals
    ///
    /// # Returns
    ///
    /// Gas cost denominated in borrow asset base units
    fn convert_gas_cost_to_borrow_asset(
        &self,
        gas_cost_usd: f64,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
    ) -> LiquidatorResult<U128> {
        // Get borrow asset price from oracle configuration
        let borrow_price_id = configuration
            .price_oracle_configuration
            .borrow_asset_price_id;
        let borrow_decimals = configuration
            .price_oracle_configuration
            .borrow_asset_decimals;

        let borrow_price = oracle_response
            .get(&borrow_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError(
                    "Borrow asset price not found in oracle".to_string(),
                )
            })?;

        // Convert price to USD value
        // Price format: price * 10^expo
        let borrow_usd = (borrow_price.price.0 as f64) * 10f64.powi(borrow_price.expo);

        // Convert gas cost from USD to borrow asset
        // gas_cost_borrow = (gas_cost_usd / borrow_usd) * 10^borrow_decimals
        let gas_cost_borrow = (gas_cost_usd / borrow_usd) * 10f64.powi(borrow_decimals);

        Ok(U128(gas_cost_borrow as u128))
    }

    /// Converts collateral asset amount to borrow asset units using oracle prices.
    ///
    /// Formula: borrow_value = (collateral_amount * collateral_usd_price) / borrow_usd_price
    ///
    /// # Arguments
    ///
    /// * `collateral_amount` - Amount in collateral asset base units
    /// * `oracle_response` - Oracle price data containing both asset prices
    /// * `configuration` - Market configuration containing price IDs and decimals
    ///
    /// # Returns
    ///
    /// Collateral value denominated in borrow asset base units
    fn convert_collateral_to_borrow_asset(
        &self,
        collateral_amount: U128,
        oracle_response: &OracleResponse,
        configuration: &MarketConfiguration,
    ) -> LiquidatorResult<U128> {
        let oracle_config = &configuration.price_oracle_configuration;

        // Get collateral price
        let collateral_price = oracle_response
            .get(&oracle_config.collateral_asset_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError(
                    "Collateral asset price not found in oracle".to_string(),
                )
            })?;

        // Get borrow price
        let borrow_price = oracle_response
            .get(&oracle_config.borrow_asset_price_id)
            .and_then(|opt| opt.as_ref())
            .ok_or_else(|| {
                LiquidatorError::StrategyError(
                    "Borrow asset price not found in oracle".to_string(),
                )
            })?;

        // Convert prices to f64 for calculation
        // Price format: price * 10^expo
        let collateral_usd = (collateral_price.price.0 as f64) * 10f64.powi(collateral_price.expo);
        let borrow_usd = (borrow_price.price.0 as f64) * 10f64.powi(borrow_price.expo);

        // Convert collateral to borrow asset units
        // Step 1: Convert collateral to USD value
        let collateral_amount_f64 = collateral_amount.0 as f64;
        let collateral_decimals = oracle_config.collateral_asset_decimals;
        let collateral_value_usd = (collateral_amount_f64 / 10f64.powi(collateral_decimals)) * collateral_usd;

        // Step 2: Convert USD value to borrow asset units
        let borrow_decimals = oracle_config.borrow_asset_decimals;
        let borrow_value = (collateral_value_usd / borrow_usd) * 10f64.powi(borrow_decimals);

        Ok(U128(borrow_value as u128))
    }

    /// Gets the balance of a specific asset.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_asset_balance<A: AssetClass>(
        &self,
        asset: &FungibleAsset<A>,
    ) -> LiquidatorResult<U128> {
        let balance_action = asset.balance_of_action(&self.signer.get_account_id());

        let args: serde_json::Value = serde_json::from_slice(&balance_action.args)
            .map_err(LiquidatorError::SerializeError)?;

        let balance = view::<U128>(
            &self.client,
            asset.contract_id().into(),
            &balance_action.method_name,
            args,
        )
        .await
        .map_err(LiquidatorError::FetchBalanceError)?;

        Ok(balance)
    }

    /// Creates a transfer transaction for liquidation.
    #[tracing::instrument(skip(self), level = "debug")]
    fn create_transfer_tx(
        &self,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        borrow_account: &AccountId,
        liquidation_amount: U128,
        collateral_amount: Option<U128>,
        nonce: u64,
        block_hash: CryptoHash,
    ) -> LiquidatorResult<Transaction> {
        let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
            account_id: borrow_account.clone(),
            amount: collateral_amount.map(Into::into),
        }))?;

        let function_call =
            borrow_asset.transfer_call_action(&self.market, liquidation_amount.into(), &msg);

        Ok(Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: borrow_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![function_call.into()],
        }))
    }

    /// Performs a single liquidation.
    #[tracing::instrument(skip(self, position, oracle_response, configuration), level = "info", fields(
        borrower = %borrow_account,
        market = %self.market
    ))]
    pub async fn liquidate(
        &self,
        borrow_account: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
        configuration: MarketConfiguration,
    ) -> Result<LiquidationOutcome, LiquidatorError> {
        debug!(
            borrower = %borrow_account,
            collateral = %position.collateral_asset_deposit,
            "Evaluating position for liquidation"
        );

        // Check if position is liquidatable
        let Some(status) = self
            .get_borrow_status(borrow_account.clone(), &oracle_response)
            .await
            .map_err(LiquidatorError::FetchBorrowStatus)?
        else {
            debug!(borrower = %borrow_account, "Borrow status not found");
            return Ok(LiquidationOutcome::NotLiquidatable);
        };

        let BorrowStatus::Liquidation(reason) = status else {
            debug!(
                borrower = %borrow_account,
                collateral = %position.collateral_asset_deposit,
                "Position is healthy, not liquidatable"
            );
            return Ok(LiquidationOutcome::NotLiquidatable);
        };

        info!(
            borrower = %borrow_account,
            reason = ?reason,
            collateral = %position.collateral_asset_deposit,
            "Position is liquidatable"
        );

        // Dry run mode - log and skip without executing any further checks
        if self.dry_run {
            info!(
                borrower = %borrow_account,
                collateral = %position.collateral_asset_deposit,
                borrow = %position.get_borrow_asset_principal(),
                "DRY RUN: Position is liquidatable (skipping execution)"
            );
            return Ok(LiquidationOutcome::Liquidated);
        }

        // Get available balance
        let available_balance = self.get_asset_balance(self.asset.as_ref()).await?;

        debug!(
            available_balance = %available_balance.0,
            asset = %self.asset,
            "Current balance checked"
        );

        // Calculate liquidation amount using strategy
        let Some(liquidation_amount) = self.strategy.calculate_liquidation_amount(
            &position,
            &oracle_response,
            &configuration,
            available_balance,
        )?
        else {
            info!(
                borrower = %borrow_account,
                available_balance = %available_balance.0,
                "Strategy determined no liquidation should occur"
            );
            return Ok(LiquidationOutcome::NotLiquidatable);
        };

        // Calculate actual liquidation percentage for logging
        let target_percentage = self.strategy.max_liquidation_percentage();
        let total_borrow = position.get_borrow_asset_principal();
        let total_borrow_u128 = u128::from(total_borrow);
        let actual_percentage = if total_borrow_u128 > 0 {
            ((liquidation_amount.0 as f64 / total_borrow_u128 as f64) * 100.0) as u8
        } else {
            0
        };

        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            total_borrow = %total_borrow_u128,
            target_percentage = %target_percentage,
            actual_percentage = %actual_percentage,
            strategy = %self.strategy.strategy_name(),
            available_balance = %available_balance.0,
            "Liquidation amount calculated"
        );

        let borrow_asset = &configuration.borrow_asset;

        // Check NEP-245 borrow asset balance
        let borrow_asset_balance = self.get_asset_balance(borrow_asset).await?;
        info!(
            borrower = %borrow_account,
            borrow_asset = %borrow_asset,
            borrow_asset_balance = %borrow_asset_balance.0,
            liquidation_amount_needed = %liquidation_amount.0,
            "Checked NEP-245 borrow asset balance"
        );

        // Check underlying NEP-141 balance if different from borrow asset
        let underlying_balance = if self.asset.as_ref() != borrow_asset {
            let balance = self.get_asset_balance(self.asset.as_ref()).await?;
            info!(
                borrower = %borrow_account,
                underlying_asset = %self.asset,
                underlying_contract = %self.asset.contract_id(),
                underlying_balance = %balance.0,
                needed = %liquidation_amount.0,
                "Checked underlying NEP-141 balance"
            );
            balance
        } else {
            borrow_asset_balance
        };

        // Determine if we need to swap
        let swap_output_amount = if self.asset.as_ref() == borrow_asset {
            if underlying_balance >= liquidation_amount {
                U128(0)
            } else {
                U128(liquidation_amount.0 - underlying_balance.0)
            }
        } else {
            liquidation_amount
        };

        // Get swap quote if needed
        let swap_input_amount = if swap_output_amount.0 > 0 {
            info!(
                borrower = %borrow_account,
                from = %self.asset,
                to = %borrow_asset,
                output_amount = %swap_output_amount.0,
                provider = %self.swap_provider.provider_name(),
                "Requesting quote from {}",
                self.swap_provider.provider_name()
            );

            let quote = self.swap_provider
                .quote(self.asset.as_ref(), borrow_asset, swap_output_amount)
                .await
                .map_err(|e| {
                    tracing::error!(
                        borrower = %borrow_account,
                        error = ?e,
                        output_amount = %swap_output_amount.0,
                        "Failed to get swap quote"
                    );
                    LiquidatorError::SwapProviderError(e)
                })?;

            info!(
                borrower = %borrow_account,
                input_amount = %quote.0,
                output_amount = %swap_output_amount.0,
                provider = %self.swap_provider.provider_name(),
                "Received quote from {}",
                self.swap_provider.provider_name()
            );

            quote
        } else {
            U128(0)
        };

        // Convert expected collateral from collateral asset units to borrow asset units
        let collateral_amount = U128(position.collateral_asset_deposit.into());
        let expected_collateral_borrow_units = self
            .convert_collateral_to_borrow_asset(
                collateral_amount,
                &oracle_response,
                &configuration,
            )
            .unwrap_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    "Failed to convert collateral value, using raw amount"
                );
                collateral_amount
            });

        // Convert gas cost from USD to borrow asset units using oracle
        let gas_cost_borrow_asset = self
            .convert_gas_cost_to_borrow_asset(
                Self::DEFAULT_GAS_COST_USD,
                &oracle_response,
                &configuration,
            )
            .unwrap_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    "Failed to convert gas cost, using fallback estimate"
                );
                // Fallback: assume $0.05 at $1 per token = 50000 units (6 decimals)
                U128(50_000)
            });

        debug!(
            collateral_amount = %collateral_amount.0,
            collateral_value_borrow_units = %expected_collateral_borrow_units.0,
            gas_cost_usd = %Self::DEFAULT_GAS_COST_USD,
            gas_cost_borrow_asset = %gas_cost_borrow_asset.0,
            borrow_asset = %borrow_asset,
            "Converted collateral and gas cost to borrow asset units"
        );

        // Check profitability using strategy
        // All values are now in borrow asset units for accurate comparison
        let is_profitable = self.strategy.should_liquidate(
            swap_input_amount,
            liquidation_amount,
            expected_collateral_borrow_units,
            gas_cost_borrow_asset,
        )?;

        // Calculate detailed costs for logging (all in borrow asset units)
        let swap_cost = swap_input_amount.0;
        let gas_cost = gas_cost_borrow_asset.0;
        let total_cost = swap_cost + gas_cost;
        let expected_revenue = expected_collateral_borrow_units.0;
        let net_profit = if expected_revenue > total_cost {
            expected_revenue - total_cost
        } else {
            0
        };
        let profit_percentage = if total_cost > 0 {
            ((net_profit as f64 / total_cost as f64) * 100.0) as u64
        } else {
            0
        };

        info!(
            borrower = %borrow_account,
            swap_cost = %swap_cost,
            gas_cost = %gas_cost,
            total_cost = %total_cost,
            expected_revenue = %expected_revenue,
            collateral_amount = %collateral_amount.0,
            net_profit = %net_profit,
            profit_percentage = %profit_percentage,
            is_profitable = is_profitable,
            "Profitability analysis completed (all values in borrow asset units)"
        );

        if !is_profitable {
            info!(
                borrower = %borrow_account,
                swap_cost = %swap_cost,
                gas_cost = %gas_cost,
                total_cost = %total_cost,
                expected_revenue = %expected_revenue,
                net_profit = %net_profit,
                "Liquidation not profitable, skipping"
            );
            return Ok(LiquidationOutcome::Unprofitable);
        }

        // Execute swap if needed
        if swap_input_amount.0 > 0 {
            let balance = self.get_asset_balance(self.asset.as_ref()).await?;
            if balance < swap_input_amount {
                warn!(
                    borrower = %borrow_account,
                    required = %swap_input_amount.0,
                    available = %balance.0,
                    asset = %self.asset,
                    "Insufficient balance for swap"
                );
                return Err(LiquidatorError::InsufficientBalance);
            }

            info!(
                borrower = %borrow_account,
                swap_input_amount = %swap_input_amount.0,
                from_asset = %self.asset,
                to_asset = %borrow_asset,
                provider = %self.swap_provider.provider_name(),
                balance_before = %balance.0,
                "Executing swap"
            );

            let swap_start = std::time::Instant::now();
            match self
                .swap_provider
                .swap(self.asset.as_ref(), borrow_asset, swap_input_amount)
                .await
            {
                Ok(_) => {
                    let swap_duration = swap_start.elapsed();
                    info!(
                        borrower = %borrow_account,
                        swap_duration_ms = swap_duration.as_millis(),
                        provider = %self.swap_provider.provider_name(),
                        "Swap executed successfully"
                    );
                }
                Err(e) => {
                    error!(
                        borrower = %borrow_account,
                        error = ?e,
                        provider = %self.swap_provider.provider_name(),
                        "Swap failed"
                    );
                    return Err(LiquidatorError::SwapProviderError(e));
                }
            }
        } else {
            debug!(
                borrower = %borrow_account,
                "No swap needed, sufficient balance available"
            );
        }

        // Ensure bot account is registered with collateral token contract to receive liquidation proceeds
        let collateral_asset = FungibleAsset::<CollateralAsset>::from_str(&configuration.collateral_asset.to_string())
            .map_err(|e| LiquidatorError::StrategyError(format!("Failed to parse collateral asset: {}", e)))?;

        info!(
            borrower = %borrow_account,
            collateral_asset = %collateral_asset,
            bot_account = %self.signer.get_account_id(),
            "Ensuring bot is registered with collateral token contract"
        );

        if let Err(e) = self
            .swap_provider
            .ensure_storage_registration(&collateral_asset, &self.signer.get_account_id())
            .await
        {
            warn!(
                borrower = %borrow_account,
                error = ?e,
                collateral_asset = %collateral_asset,
                "Failed to register with collateral token contract, proceeding anyway (may already be registered)"
            );
        }

        // Execute liquidation
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer)
            .await
            .map_err(LiquidatorError::AccessKeyDataError)?;

        let tx = self.create_transfer_tx(
            borrow_asset,
            &borrow_account,
            liquidation_amount,
            None, // Let contract calculate collateral amount
            nonce,
            block_hash,
        )?;

        info!(
            borrower = %borrow_account,
            liquidation_amount = %liquidation_amount.0,
            expected_collateral_borrow_units = %expected_collateral_borrow_units.0,
            collateral_amount = %collateral_amount.0,
            "Submitting liquidation transaction"
        );

        let tx_start = std::time::Instant::now();
        match send_tx(&self.client, &self.signer, self.timeout, tx).await {
            Ok(_) => {
                let tx_duration = tx_start.elapsed();
                info!(
                    borrower = %borrow_account,
                    liquidation_amount = %liquidation_amount.0,
                    expected_collateral_borrow_units = %expected_collateral_borrow_units.0,
                    collateral_amount = %collateral_amount.0,
                    tx_duration_ms = tx_duration.as_millis(),
                    "✅ Liquidation executed successfully"
                );
            }
            Err(e) => {
                error!(
                    borrower = %borrow_account,
                    liquidation_amount = %liquidation_amount.0,
                    error = ?e,
                    "❌ Liquidation transaction failed"
                );
                return Err(LiquidatorError::LiquidationTransactionError(e));
            }
        }

        Ok(LiquidationOutcome::Liquidated)
    }

    /// Runs liquidations for all eligible positions in the market.
    ///
    /// # Arguments
    ///
    /// * `_concurrency` - Maximum number of concurrent liquidations (currently unused - sequential processing)
    #[tracing::instrument(skip(self, _concurrency), level = "info", fields(market = %self.market))]
    pub async fn run_liquidations(&self, _concurrency: usize) -> LiquidatorResult {
        info!(
            strategy = %self.strategy.strategy_name(),
            target_percentage = %self.strategy.max_liquidation_percentage(),
            swap_provider = %self.swap_provider.provider_name(),
            "Starting liquidation run"
        );

        // Check if market is compatible before proceeding
        if !self.is_market_compatible().await? {
            return Ok(()); // Skip incompatible markets
        }

        let configuration = self.get_configuration().await?;

        info!(
            borrow_asset = %configuration.borrow_asset,
            collateral_asset = %configuration.collateral_asset,
            borrow_mcr = %configuration.borrow_mcr_maintenance.to_string(),
            "Market configuration loaded"
        );

        let oracle_response = self
            .get_oracle_prices(
                configuration.price_oracle_configuration.account_id.clone(),
                &[
                    configuration
                        .price_oracle_configuration
                        .borrow_asset_price_id,
                    configuration
                        .price_oracle_configuration
                        .collateral_asset_price_id,
                ],
                configuration.price_oracle_configuration.price_maximum_age_s,
            )
            .await?;

        // Check if oracle returned empty prices (market skipped due to oracle incompatibility)
        if oracle_response.is_empty() {
            return Ok(());
        }

        // Log oracle prices for visibility
        debug!(
            borrow_price_id = ?configuration.price_oracle_configuration.borrow_asset_price_id,
            collateral_price_id = ?configuration.price_oracle_configuration.collateral_asset_price_id,
            oracle_account = %configuration.price_oracle_configuration.account_id,
            max_age_s = configuration.price_oracle_configuration.price_maximum_age_s,
            "Oracle prices fetched"
        );

        let borrows = self.get_borrows().await?;

        if borrows.is_empty() {
            tracing::info!("No borrow positions found");
            return Ok(());
        }

        tracing::info!(
            positions = borrows.len(),
            borrow_asset = %configuration.borrow_asset,
            collateral_asset = %configuration.collateral_asset,
            "Found borrow positions to evaluate"
        );

        // Record configuration in span
        Span::current().record(
            "borrow_asset",
            configuration.borrow_asset.to_string().as_str(),
        );
        Span::current().record(
            "collateral_asset",
            configuration.collateral_asset.to_string().as_str(),
        );

        let start_time = std::time::Instant::now();
        let total_positions = borrows.len();
        let mut liquidated_count = 0u32;
        let mut not_liquidatable_count = 0u32;
        let mut failed_count = 0u32;
        let mut skipped_unprofitable = 0u32;

        for (i, (account, position)) in borrows.into_iter().enumerate() {
            let result = self
                .liquidate(
                    account.clone(),
                    position.clone(),
                    oracle_response.clone(),
                    configuration.clone(),
                )
                .await;

            match result {
                Ok(outcome) => match outcome {
                    LiquidationOutcome::Liquidated => {
                        liquidated_count += 1;
                    }
                    LiquidationOutcome::NotLiquidatable => {
                        not_liquidatable_count += 1;
                    }
                    LiquidationOutcome::Unprofitable => {
                        skipped_unprofitable += 1;
                    }
                },
                Err(e) => {
                    if let LiquidatorError::InsufficientBalance = &e {
                        warn!(borrower = %account, "Insufficient balance for liquidation");
                        failed_count += 1;
                    } else {
                        debug!(borrower = %account, error = ?e, "Liquidation attempt failed");
                        failed_count += 1;
                    }
                }
            }

            // Add delay between positions to avoid rate limiting (except after last position)
            if i < total_positions - 1 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }

        let elapsed = start_time.elapsed();
        info!(
            duration_ms = elapsed.as_millis(),
            duration_s = elapsed.as_secs(),
            total_positions = total_positions,
            liquidated = liquidated_count,
            not_liquidatable = not_liquidatable_count,
            skipped_unprofitable = skipped_unprofitable,
            failed = failed_count,
            "Liquidation run completed"
        );

        Ok(())
    }
}

// Re-export types for CLI arguments
use crate::rpc::Network;
use clap::ValueEnum;

/// Swap provider types available for liquidation.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SwapType {
    /// Rhea Finance DEX
    RheaSwap,
    /// 1-Click API (NEAR Intents)
    OneClickApi,
}

impl SwapType {
    /// Returns the contract account ID for the swap provider.
    #[must_use]
    #[allow(
        clippy::unwrap_used,
        reason = "We know the contract IDs are valid NEAR account IDs."
    )]
    pub fn account_id(self, network: Network) -> AccountId {
        match self {
            SwapType::RheaSwap => match network {
                Network::Mainnet => "dclv2.ref-labs.near".parse().unwrap(),
                Network::Testnet => "dclv2.ref-dev.testnet".parse().unwrap(),
            },
            SwapType::OneClickApi => match network {
                Network::Mainnet => "intents.near".parse().unwrap(),
                Network::Testnet => "intents.testnet".parse().unwrap(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
