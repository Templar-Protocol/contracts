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

use std::{collections::HashMap, sync::Arc};

use futures::{StreamExt, TryStreamExt};
use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
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
    asset::{AssetClass, BorrowAsset, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    market::{DepositMsg, LiquidateMsg, MarketConfiguration},
    oracle::pyth::{OracleResponse, PriceIdentifier},
};
use tracing::{debug, error, info, instrument, warn};

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
    /// Estimated gas cost per liquidation (in yoctoNEAR)
    gas_cost_estimate: U128,
}

impl Liquidator {
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        asset: Arc<FungibleAsset<BorrowAsset>>,
        market: AccountId,
        swap_provider: SwapProviderImpl,
        strategy: Box<dyn LiquidationStrategy>,
        timeout: u64,
    ) -> Self {
        Self {
            client,
            signer,
            asset,
            market,
            swap_provider,
            strategy,
            timeout,
            gas_cost_estimate: Self::DEFAULT_GAS_COST_ESTIMATE,
        }
    }

    /// Default gas cost estimate: ~0.01 NEAR
    const DEFAULT_GAS_COST_ESTIMATE: U128 = U128(10_000_000_000_000_000_000_000);

    /// Fetches the market configuration.
    #[instrument(skip(self), level = "debug")]
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
    #[instrument(skip(self), level = "debug")]
    async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        view(
            &self.client,
            oracle,
            "list_ema_prices_no_older_than",
            json!({ "price_ids": price_ids, "age": age }),
        )
        .await
        .map_err(LiquidatorError::PriceFetchError)
    }

    /// Fetches borrow status for an account.
    #[instrument(skip(self), level = "debug")]
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
    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> LiquidatorResult<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let page = view::<BorrowPositions>(
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

    /// Gets the balance of a specific asset.
    #[instrument(skip(self), level = "debug")]
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
    #[instrument(skip(self), level = "debug")]
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
    #[instrument(skip(self, position, oracle_response, configuration), level = "info", fields(
        borrower = %borrow_account,
        market = %self.market
    ))]
    pub async fn liquidate(
        &self,
        borrow_account: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
        configuration: MarketConfiguration,
    ) -> LiquidatorResult {
        // Check if position is liquidatable
        let Some(status) = self
            .get_borrow_status(borrow_account.clone(), &oracle_response)
            .await
            .map_err(LiquidatorError::FetchBorrowStatus)?
        else {
            debug!("Borrow status not found");
            return Ok(());
        };

        let BorrowStatus::Liquidation(reason) = status else {
            debug!("Position is not liquidatable");
            return Ok(());
        };

        info!(?reason, "Position is liquidatable");

        // Get available balance
        let available_balance = self.get_asset_balance(self.asset.as_ref()).await?;

        // Calculate liquidation amount using strategy
        let Some(liquidation_amount) = self.strategy.calculate_liquidation_amount(
            &position,
            &oracle_response,
            &configuration,
            available_balance,
        )?
        else {
            info!("Strategy determined no liquidation should occur");
            return Ok(());
        };

        info!(
            amount = %liquidation_amount.0,
            strategy = %self.strategy.strategy_name(),
            "Liquidation amount calculated"
        );

        let borrow_asset = &configuration.borrow_asset;

        // Determine if we need to swap
        let swap_output_amount = if self.asset.as_ref() == borrow_asset {
            let asset_balance = self.get_asset_balance(self.asset.as_ref()).await?;
            if asset_balance >= liquidation_amount {
                U128(0)
            } else {
                U128(liquidation_amount.0 - asset_balance.0)
            }
        } else {
            liquidation_amount
        };

        // Get swap quote if needed
        let swap_input_amount = if swap_output_amount.0 > 0 {
            self.swap_provider
                .quote(self.asset.as_ref(), borrow_asset, swap_output_amount)
                .await
                .map_err(LiquidatorError::SwapProviderError)?
        } else {
            U128(0)
        };

        // Calculate expected collateral (simplified - in production, use price oracle)
        let expected_collateral = U128(position.collateral_asset_deposit.into());

        // Check profitability using strategy
        if !self.strategy.should_liquidate(
            swap_input_amount,
            liquidation_amount,
            expected_collateral,
            self.gas_cost_estimate,
        )? {
            info!("Liquidation not profitable, skipping");
            return Ok(());
        }

        // Execute swap if needed
        if swap_input_amount.0 > 0 {
            let balance = self.get_asset_balance(self.asset.as_ref()).await?;
            if balance < swap_input_amount {
                warn!(
                    required = %swap_input_amount.0,
                    available = %balance.0,
                    "Insufficient balance for swap"
                );
                return Err(LiquidatorError::InsufficientBalance);
            }

            info!(
                amount = %swap_input_amount.0,
                provider = %self.swap_provider.provider_name(),
                "Executing swap"
            );

            match self
                .swap_provider
                .swap(self.asset.as_ref(), borrow_asset, swap_input_amount)
                .await
            {
                Ok(_) => info!("Swap executed successfully"),
                Err(e) => {
                    error!(?e, "Swap failed");
                    return Err(LiquidatorError::SwapProviderError(e));
                }
            }
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

        info!("Submitting liquidation transaction");

        match send_tx(&self.client, &self.signer, self.timeout, tx).await {
            Ok(_) => {
                info!(
                    liquidation_amount = %liquidation_amount.0,
                    "Liquidation executed successfully"
                );
            }
            Err(e) => {
                error!(?e, "Liquidation transaction failed");
                return Err(LiquidatorError::LiquidationTransactionError(e));
            }
        }

        Ok(())
    }

    /// Runs liquidations for all eligible positions in the market.
    ///
    /// # Arguments
    ///
    /// * `concurrency` - Maximum number of concurrent liquidations
    #[instrument(skip(self), level = "info", fields(market = %self.market))]
    pub async fn run_liquidations(&self, concurrency: usize) -> LiquidatorResult {
        info!(
            strategy = %self.strategy.strategy_name(),
            swap_provider = %self.swap_provider.provider_name(),
            "Starting liquidation run"
        );

        let configuration = self.get_configuration().await?;
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

        let borrows = self.get_borrows().await?;

        if borrows.is_empty() {
            info!("No borrow positions found");
            return Ok(());
        }

        info!(positions = borrows.len(), "Found borrow positions");

        futures::stream::iter(borrows)
            .map(|(account, position)| {
                let oracle_response = oracle_response.clone();
                let configuration = configuration.clone();
                async move {
                    self.liquidate(account, position, oracle_response, configuration)
                        .await
                }
            })
            .buffer_unordered(concurrency)
            .try_for_each(|()| async { Ok(()) })
            .await?;

        info!("Liquidation run completed");

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
    /// NEAR Intents cross-chain
    NearIntents,
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
            SwapType::NearIntents => match network {
                Network::Mainnet => "intents.near".parse().unwrap(),
                Network::Testnet => "intents.testnet".parse().unwrap(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
