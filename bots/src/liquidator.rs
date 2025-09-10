use std::{collections::HashMap, sync::Arc};

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId,
};
use templar_common::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAsset, FungibleAssetParseError},
    borrow::{BorrowPosition, BorrowStatus},
    market::{error::RetrievalError, DepositMsg, LiquidateMsg, MarketConfiguration},
    oracle::pyth::{OracleResponse, PriceIdentifier},
};
use tracing::{error, info, instrument, warn};

use crate::{
    near::{get_access_key_data, send_tx, view, AppError, RpcError},
    swap::{Swap, SwapType},
    BorrowPositions, Network,
};

/// Errors that can occur during liquidation operations.
#[derive(Debug, thiserror::Error)]
pub enum LiquidatorError {
    /// Error while fetching borrow status.
    #[error("Failed to fetch borrow status: {0}")]
    FetchBorrowStatus(RpcError),
    /// Error serializing data.
    #[error("Failed to serialize data: {0}")]
    SerializeError(#[from] serde_json::Error),
    /// Price pair retrieval error.
    #[error("Failed to get price pair: {0}")]
    PricePairError(#[from] RetrievalError),
    /// Error calculating minimum acceptable liquidation amount.
    #[error("Failed to calculate minimum acceptable liquidation amount: {0}")]
    MinimumLiquidationAmountError(String),
    /// Standart support error.
    #[error("Standard support error: {0}")]
    StandardSupportError(String),
    /// Quote error.
    #[error("Failed to get quote: {0}")]
    QuoteError(AppError),
    /// Error fetching market configuration.
    #[error("Failed to get market configuration: {0}")]
    GetConfigurationError(RpcError),
    /// Error fetching oracle prices.
    #[error("Failed to fetch oracle prices: {0}")]
    PriceFetchError(RpcError),
    /// Access key data retrieval error.
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(RpcError),
    /// Swap transaction error.
    #[error("Swap transaction error: {0}")]
    SwapTransactionError(AppError),
    /// Liquidation transaction error.
    #[error("Liquidation transaction error: {0}")]
    LiquidationTransactionError(RpcError),
    /// Error while fetching borrow positions.
    #[error("Failed to list borrow positions: {0}")]
    ListBorrowPositionsError(RpcError),
    /// Error fetching registry deployments.
    #[error("Failed to list deployments: {0}")]
    ListDeploymentsError(RpcError),
    /// Error fetching minimum acceptable liquidation amount.
    #[error("Failed to fetch balance: {0}")]
    FetchBalanceError(RpcError),
    /// Asset parsing error.
    #[error("Asset parsing error: {0}")]
    AssetParseError(FungibleAssetParseError),
}

pub type LiquidatorResult<T = ()> = Result<T, LiquidatorError>;

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Market registries to run liquidations for
    #[arg(short, long, env = "REGISTRY_ACCOUNT_IDS")]
    pub registries: Vec<AccountId>,
    /// Swap to use for liquidations
    #[arg(long, env = "SWAP_TYPE")]
    pub swap: SwapType,
    /// Signer key to use for signing transactions.
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: SecretKey,
    /// Signer `AccountId`.
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Asset specification (NEP-141 or NEP-245) to liquidate with - "nep141:contract.near" (NEP-141) or "nep245:contract.near:token_id" (NEP-245)
    #[arg(short, long, env = "ASSET_SPEC")]
    pub asset: FungibleAsset<BorrowAsset>,
    /// Network to run liquidations on
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Timeout for transactions
    #[arg(short, long, env = "TIMEOUT", default_value_t = 60)]
    pub timeout: u64,
    /// Interval between liquidation attempts
    #[arg(short, long, env = "INTERVAL", default_value_t = 600)]
    pub interval: u64,
    /// Registry refresh interval in seconds
    #[arg(long, env = "REGISTRY_REFRESH_INTERVAL", default_value_t = 3600)]
    pub registry_refresh_interval: u64,
    /// Concurency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,
}

pub struct Liquidator<S: Swap> {
    client: JsonRpcClient,
    signer: Arc<InMemorySigner>,
    asset: Arc<FungibleAsset<BorrowAsset>>,
    pub market: AccountId,
    timeout: u64,
    swap: Arc<S>,
}

impl<S: Swap> Liquidator<S> {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<InMemorySigner>,
        asset: Arc<FungibleAsset<BorrowAsset>>,
        market: AccountId,
        swap: Arc<S>,
        timeout: u64,
    ) -> Self {
        Self {
            client,
            signer,
            asset,
            market,
            timeout,
            swap,
        }
    }

    /// Gets the asset specification for testing purposes.
    #[cfg(test)]
    pub fn asset(&self) -> &FungibleAsset<BorrowAsset> {
        &self.asset
    }

    /// Gets the timeout for testing purposes.
    #[cfg(test)]
    pub fn timeout(&self) -> u64 {
        self.timeout
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrow_status(
        &self,
        borrow: AccountId,
        oracle_response: &OracleResponse,
    ) -> LiquidatorResult<Option<BorrowStatus>> {
        view(
            &self.client,
            self.market.clone(),
            "get_borrow_status",
            &json!({
                "account_id": borrow,
                "oracle_response": oracle_response,
            }),
        )
        .await
        .map_err(LiquidatorError::FetchBorrowStatus)
    }

    /// Converts a market configuration borrow asset to `FungibleAsset`.
    fn borrow_asset_to_spec(configuration: &MarketConfiguration) -> FungibleAsset<BorrowAsset> {
        configuration.borrow_asset.clone()
    }

    /// Converts a market configuration collateral asset to `FungibleAsset`.
    fn collateral_asset_to_spec(configuration: &MarketConfiguration) -> FungibleAsset<CollateralAsset> {
        configuration.collateral_asset.clone()
    }

    /// Creates a transfer transaction for liquidation.
    ///
    /// # Errors
    ///
    /// Returns `LiquidatorError::SerializationError` if message serialization fails,
    /// or `LiquidatorError::TransactionBuildError` if transaction building fails.
    pub fn create_transfer_tx(
        &self,
        borrow_asset: &FungibleAsset<BorrowAsset>,
        borrow: &AccountId,
        liquidation_amount: U128,
        nonce: u64,
        block_hash: CryptoHash,
    ) -> LiquidatorResult<Transaction> {
        let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
            account_id: borrow.clone(),
        }))?;

        let function_call = borrow_asset.transfer_call_action(&self.market, liquidation_amount.into(), &msg);

        Ok(Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: borrow_asset.contract_id(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(function_call))],
        }))
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn liquidate(
        &self,
        borrow: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
        configuration: MarketConfiguration,
    ) -> LiquidatorResult {
        let Some(status) = self
            .get_borrow_status(borrow.clone(), &oracle_response)
            .await?
        else {
            info!("Borrow status not found");
            return Ok(());
        };

        let BorrowStatus::Liquidation(reason) = status else {
            info!("Borrow status is not liquidation");
            return Ok(());
        };

        info!("Liquidation reason: {reason:?}");

        let borrow_asset = Self::borrow_asset_to_spec(&configuration);
        let collateral_asset = Self::collateral_asset_to_spec(&configuration);

        let liquidation_amount = self
            .liquidation_amount(&position, &oracle_response, configuration)
            .await?;

        let swap_output_amount = if self.asset.as_ref() == &borrow_asset {
            let asset_balance = self.get_asset_balance(self.asset.as_ref()).await?;
            if asset_balance >= liquidation_amount {
                0.into()
            } else {
                (liquidation_amount.0 - asset_balance.0).into()
            }
        } else {
            liquidation_amount
        };

        let swap_amount = self
            .swap
            .quote(self.asset.as_ref(), &borrow_asset, swap_output_amount)
            .await
            .map_err(LiquidatorError::QuoteError)?;

        let available = self.get_asset_balance(self.asset.as_ref()).await?;

        if available < swap_amount {
            warn!("Insufficient asset balance for liquidation: {available:?} < {swap_amount:?}");
            return Ok(());
        }

        // Implement this function based on your liquidation strategy
        if !self
            .should_liquidate(swap_amount, liquidation_amount)
            .await?
        {
            info!("Skipping liquidation due to insufficient conditions");
            return Ok(());
        }

        if swap_amount > 0.into() {
            match self
                .swap
                .swap(self.asset.as_ref(), &borrow_asset, swap_amount)
                .await
            {
                Ok(_) => {
                    info!("Swap successful");
                }
                Err(e) => {
                    error!("Swap failed: {e}");
                    return Err(LiquidatorError::SwapTransactionError(e));
                }
            };
        }

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer)
            .await
            .map_err(LiquidatorError::AccessKeyDataError)?;

        let transfer_call_tx = self.create_transfer_tx(
            &borrow_asset,
            &borrow,
            liquidation_amount,
            nonce,
            block_hash,
        )?;

        match send_tx(&self.client, &self.signer, self.timeout, transfer_call_tx).await {
            Ok(_) => {
                info!("Liquidation successful");
            }
            Err(e) => {
                error!("Liquidation failed: {e}");
                return Err(LiquidatorError::LiquidationTransactionError(e));
            }
        }

        if self.asset.contract_id() == collateral_asset.contract_id() {
            match self
                .swap
                .swap(
                    &collateral_asset,
                    self.asset.as_ref(),
                    position.collateral_asset_deposit.into(),
                )
                .await
            {
                Ok(_) => {
                    info!("Swap successful");
                }
                Err(e) => {
                    error!("Swap failed: {e}");
                }
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    async fn liquidation_amount(
        &self,
        position: &BorrowPosition,
        oracle_response: &OracleResponse,
        configuration: MarketConfiguration,
    ) -> LiquidatorResult<U128> {
        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)?;
        let min_liquidation_amount = configuration
            .minimum_acceptable_liquidation_amount(position.collateral_asset_deposit, &price_pair)
            .ok_or_else(|| {
                LiquidatorError::MinimumLiquidationAmountError(
                    "Failed to calculate minimum acceptable liquidation amount".to_owned(),
                )
            })?;
        Ok(min_liquidation_amount.into())
    }

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

    #[instrument(skip(self), level = "debug")]
    async fn get_asset_balance<A: AssetClass>(&self, asset: &FungibleAsset<A>) -> LiquidatorResult<U128> {
        let balance_action = asset.balance_of_action(&self.signer.account_id);
        
        let args: serde_json::Value = serde_json::from_slice(&balance_action.args)
            .expect("Balance action args should be valid JSON");
        
        let balance = view::<U128>(
            &self.client,
            asset.contract_id(),
            &balance_action.method_name,
            args,
        )
        .await
        .map_err(LiquidatorError::FetchBalanceError)?;

        Ok(balance)
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> LiquidatorResult<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();
        let page_size = 500;
        let mut current_offset = 0;

        loop {
            let params = json!({
                "offset": current_offset,
                "count": page_size,
            });

            let page = view::<BorrowPositions>(
                &self.client,
                self.market.clone(),
                "list_borrow_positions",
                params,
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

    #[instrument(skip(self), level = "info")]
    pub async fn run_liquidations(&self, concurrency: usize) -> LiquidatorResult {
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

        futures::stream::iter(borrows)
            .map(|(borrow, position)| {
                let oracle_response = oracle_response.clone();
                let configuration = configuration.clone();
                async move {
                    self.liquidate(borrow, position, oracle_response, configuration)
                        .await
                }
            })
            .buffer_unordered(concurrency)
            .try_for_each(|_result| async { Ok(()) })
            .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn should_liquidate(
        &self,
        swap_amount: U128,
        liquidation_amount: U128,
    ) -> LiquidatorResult<bool> {
        // TODO: Calculate optimal liquidation amount
        // For purposes of this example implementation we will just use the minimum acceptable
        // liquidation amount.
        // Costs to take into account here are:
        //  - Gas fees
        //  - Price impact
        //  - Slippage
        // All of this would be used in calculating both the optimal liquidation amount and wether to
        // perform full or partial liquidation
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_common::asset::{BorrowAsset, FungibleAsset, FungibleAssetParseError};

    #[test]
    fn test_asset_spec_nep141_parsing() {
        let spec: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        assert!(spec.clone().into_nep141().is_some());
        assert!(spec.clone().into_nep245().is_none());
        assert_eq!(
            spec.contract_id(),
            "token.near".parse::<AccountId>().unwrap()
        );
        assert_eq!(spec.to_string(), "nep141:token.near");
    }

    #[test]
    fn test_asset_spec_nep245_parsing() {
        let spec: FungibleAsset<BorrowAsset> = "nep245:multi.near:token123".parse().unwrap();
        assert!(spec.clone().into_nep141().is_none());
        assert!(spec.clone().into_nep245().is_some());
        assert_eq!(
            spec.contract_id(),
            "multi.near".parse::<AccountId>().unwrap()
        );
        assert_eq!(spec.to_string(), "nep245:multi.near:token123");
    }

    #[test]
    fn test_asset_spec_invalid_format() {
        assert!(matches!(
            "invalid".parse::<FungibleAsset<BorrowAsset>>(),
            Err(FungibleAssetParseError::InvalidFormat)
        ));
        assert!(matches!(
            "nep141".parse::<FungibleAsset<BorrowAsset>>(),
            Err(FungibleAssetParseError::InvalidFormat)
        ));
        assert!(matches!(
            "nep245:contract".parse::<FungibleAsset<BorrowAsset>>(),
            Err(FungibleAssetParseError::InvalidFormat)
        ));
    }

    #[test]
    fn test_asset_spec_invalid_account_id() {
        assert!(matches!(
            "nep141:a".parse::<FungibleAsset<BorrowAsset>>(),
            Err(FungibleAssetParseError::InvalidAccountId(_))
        ));
    }

    #[test]
    fn test_asset_spec_empty_token_id() {
        assert!(matches!(
            "nep245:contract.near:".parse::<FungibleAsset<BorrowAsset>>(),
            Err(FungibleAssetParseError::EmptyTokenId)
        ));
    }

    #[test]
    fn test_asset_methods() {
        let nep141_spec: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        assert!(nep141_spec.clone().into_nep141().is_some());
        assert!(nep141_spec.clone().into_nep245().is_none());

        let nep245_spec: FungibleAsset<BorrowAsset> = "nep245:multi.near:token123".parse().unwrap();
        assert!(nep245_spec.clone().into_nep141().is_none());
        assert!(nep245_spec.clone().into_nep245().is_some());
    }

    #[test]
    fn test_asset_compatibility() {
        let nep141_spec: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        let account_id: AccountId = "user.near".parse().unwrap();

        // Test that we can get the balance action
        let balance_action = nep141_spec.balance_of_action(&account_id);
        assert_eq!(balance_action.method_name, "ft_balance_of");

        let nep245_spec: FungibleAsset<BorrowAsset> = "nep245:multi.near:token123".parse().unwrap();
        let balance_action = nep245_spec.balance_of_action(&account_id);
        assert_eq!(balance_action.method_name, "mt_balance_of");
    }

    #[test]
    fn test_fungible_asset_compatibility() {
        // Test that we can create FungibleAsset directly
        let fungible_asset =
            FungibleAsset::<BorrowAsset>::nep141("token.near".parse::<AccountId>().unwrap());

        assert!(fungible_asset.clone().into_nep141().is_some());
        assert_eq!(
            fungible_asset.contract_id(),
            "token.near".parse::<AccountId>().unwrap()
        );
        assert_eq!(fungible_asset.to_string(), "nep141:token.near");

        // Test NEP-245
        let fungible_asset = FungibleAsset::<BorrowAsset>::nep245(
            "multi.near".parse::<AccountId>().unwrap(),
            "token123".to_string(),
        );

        assert!(fungible_asset.clone().into_nep245().is_some());
        assert_eq!(
            fungible_asset.contract_id(),
            "multi.near".parse::<AccountId>().unwrap()
        );
        assert_eq!(fungible_asset.to_string(), "nep245:multi.near:token123");
    }
}
