use std::{collections::HashMap, sync::Arc};

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::{InMemorySigner, SecretKey};
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
    asset::{AssetClass, BorrowAsset, FungibleAsset, FungibleAssetParseError},
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
    /// Asset specification (NEP-141 or NEP-245) to liquidate with - "nep141:contract.near" (NEP-141) or "`nep245:contract.near:token_id`" (NEP-245)
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
        account_id: AccountId,
        oracle_response: &OracleResponse,
    ) -> Result<Option<BorrowStatus>, RpcError> {
        let params = json!({
            "account_id": account_id,
            "oracle_response": oracle_response,
        });

        let result = view(
            &self.client,
            self.market.clone(),
            "get_borrow_status",
            &params,
        )
        .await?;

        Ok(result)
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
            // TODO: This should be an amount expected to receive
            amount: None,
        }))?;

        let function_call =
            borrow_asset.transfer_call_action(&self.market, liquidation_amount.into(), &msg);

        Ok(Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: borrow_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![function_call.into()],
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
            .await
            .map_err(LiquidatorError::FetchBorrowStatus)?
        else {
            info!("Borrow status not found");
            return Ok(());
        };

        let BorrowStatus::Liquidation(reason) = status else {
            info!("Borrow status is not liquidation");
            return Ok(());
        };

        info!("Liquidation reason: {reason:?}");

        let liquidation_amount = self
            .liquidation_amount(&position, &oracle_response, &configuration)
            .await?;

        let borrow_asset = &configuration.borrow_asset;
        let collateral_asset = &configuration.collateral_asset;

        let swap_output_amount = if self.asset.as_ref() == borrow_asset {
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
            .quote(self.asset.as_ref(), borrow_asset, swap_output_amount)
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
                .swap(self.asset.as_ref(), borrow_asset, swap_amount)
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

        let transfer_call_tx =
            self.create_transfer_tx(borrow_asset, &borrow, liquidation_amount, nonce, block_hash)?;

        match send_tx(&self.client, &self.signer, self.timeout, transfer_call_tx).await {
            Ok(_) => {
                info!("Liquidation successful");
            }
            Err(e) => {
                error!("Liquidation failed: {e}");
                return Err(LiquidatorError::LiquidationTransactionError(e));
            }
        }

        if self.asset.as_ref() == &collateral_asset.clone().coerce::<BorrowAsset>() {
            match self
                .swap
                .swap(
                    collateral_asset,
                    &self.asset,
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
        configuration: &MarketConfiguration,
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
    async fn get_asset_balance<A: AssetClass>(
        &self,
        asset: &FungibleAsset<A>,
    ) -> LiquidatorResult<U128> {
        let balance_action = asset.balance_of_action(&self.signer.account_id);

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
    use crate::swap::Swap;
    use near_crypto::{InMemorySigner, SecretKey};
    use near_jsonrpc_client::JsonRpcClient;
    use near_primitives::views::FinalExecutionStatus;
    use near_sdk::{json_types::U128, AccountId};
    use std::sync::Arc;
    use templar_common::asset::{AssetClass, BorrowAsset, FungibleAsset, FungibleAssetParseError};

    /// Mock swap implementation for testing
    #[derive(Debug, Clone)]
    pub struct MockSwap {
        /// Exchange rate from input to output (e.g., 1.0 means 1:1 ratio)
        exchange_rate: f64,
    }

    impl MockSwap {
        pub fn new(exchange_rate: f64) -> Self {
            Self { exchange_rate }
        }
    }

    #[async_trait::async_trait]
    impl Swap for MockSwap {
        async fn quote<F: AssetClass, T: AssetClass>(
            &self,
            _from_asset: &FungibleAsset<F>,
            _to_asset: &FungibleAsset<T>,
            output_amount: U128,
        ) -> crate::near::AppResult<U128> {
            // Calculate input amount needed to get desired output
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let input_amount = (output_amount.0 as f64 / self.exchange_rate) as u128;
            Ok(U128(input_amount))
        }

        async fn swap<F: AssetClass, T: AssetClass>(
            &self,
            _from_asset: &FungibleAsset<F>,
            _to_asset: &FungibleAsset<T>,
            _amount: U128,
        ) -> crate::near::AppResult<FinalExecutionStatus> {
            // Mock successful swap - in real implementation this would execute the swap
            Ok(FinalExecutionStatus::SuccessValue(vec![]))
        }
    }

    #[tokio::test]
    async fn test_liquidator_bot_creation_integration() {
        // Integration test for creating a liquidator bot with realistic parameters

        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test-liquidator");
        let liquidator_account_id: AccountId = "liquidator.testnet".parse().unwrap();
        let signer = Arc::new(InMemorySigner::from_secret_key(
            liquidator_account_id,
            signer_key,
        ));
        let market_id: AccountId = "market.testnet".parse().unwrap();
        let swap = Arc::new(MockSwap::new(1.0));

        // Test with NEP-141 asset (USDC-like token)
        let usdc_asset = Arc::new(FungibleAsset::<BorrowAsset>::nep141(
            "usdc.testnet".parse().unwrap(),
        ));

        let liquidator = Liquidator::new(
            client.clone(),
            signer.clone(),
            usdc_asset.clone(),
            market_id.clone(),
            swap.clone(),
            120, // 2 minute timeout
        );

        // Verify liquidator properties
        assert_eq!(liquidator.asset(), &*usdc_asset);
        assert_eq!(liquidator.timeout(), 120);
        assert_eq!(liquidator.market, market_id);

        println!("✓ USDC liquidator bot created successfully");

        // Test with NEP-245 asset (multi-token)
        let mt_asset = Arc::new(FungibleAsset::<BorrowAsset>::nep245(
            "multitoken.testnet".parse().unwrap(),
            "eth".to_string(),
        ));

        let mt_liquidator = Liquidator::new(
            client,
            signer,
            mt_asset.clone(),
            market_id.clone(),
            swap,
            60,
        );

        assert_eq!(mt_liquidator.asset(), &*mt_asset);
        assert_eq!(mt_liquidator.timeout(), 60);

        println!("✓ Multi-token liquidator bot created successfully");
        println!("✓ Liquidator bot integration test completed");
    }

    #[tokio::test]
    async fn test_liquidator_bot_should_liquidate_logic() {
        // Test the liquidator's decision-making logic

        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test-liquidator");
        let liquidator_account_id: AccountId = "liquidator.testnet".parse().unwrap();
        let signer = Arc::new(InMemorySigner::from_secret_key(
            liquidator_account_id,
            signer_key,
        ));
        let market_id: AccountId = "market.testnet".parse().unwrap();
        let swap = Arc::new(MockSwap::new(1.0));

        let usdc_asset = Arc::new(FungibleAsset::<BorrowAsset>::nep141(
            "usdc.testnet".parse().unwrap(),
        ));

        let liquidator = Liquidator::new(client, signer, usdc_asset, market_id, swap, 60);

        // Test should_liquidate logic with different amounts
        let small_swap_amount = U128(100);
        let small_liquidation_amount = U128(200);

        let should_liquidate_small = liquidator
            .should_liquidate(small_swap_amount, small_liquidation_amount)
            .await
            .unwrap();

        assert!(should_liquidate_small, "Should liquidate small amounts");

        let large_swap_amount = U128(10_000);
        let large_liquidation_amount = U128(20_000);

        let should_liquidate_large = liquidator
            .should_liquidate(large_swap_amount, large_liquidation_amount)
            .await
            .unwrap();

        assert!(should_liquidate_large, "Should liquidate large amounts");

        println!("✓ Liquidator decision logic working correctly");
    }

    #[test]
    fn test_liquidator_creation() {
        // Test that we can create a liquidator instance with different configurations

        // Setup mock components
        let client = JsonRpcClient::connect("http://localhost:3030");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test-key");
        let liquidator_account_id: AccountId = "liquidator.test.near".parse().unwrap();
        let signer = Arc::new(InMemorySigner::from_secret_key(
            liquidator_account_id,
            signer_key,
        ));
        let market_id: AccountId = "market.test.near".parse().unwrap();
        let swap = Arc::new(MockSwap::new(1.0));

        // Test NEP-141 asset
        let nep141_asset = Arc::new(FungibleAsset::<BorrowAsset>::nep141(
            "token.near".parse().unwrap(),
        ));

        let _liquidator = Liquidator::new(
            client.clone(),
            signer.clone(),
            nep141_asset.clone(),
            market_id.clone(),
            swap.clone(),
            60,
        );

        // Verify liquidator was created successfully
        // Note: We can't directly access private fields, but we can verify the liquidator
        // was constructed without panicking
        println!("Liquidator created successfully");

        // Test NEP-245 asset
        let nep245_asset = Arc::new(FungibleAsset::<BorrowAsset>::nep245(
            "multi.near".parse().unwrap(),
            "token123".to_string(),
        ));

        let _liquidator_mt = Liquidator::new(
            client,
            signer,
            nep245_asset,
            market_id,
            swap,
            120, // Different timeout
        );

        // Verify multi-token liquidator was created successfully
        println!("Multi-token liquidator created successfully");
    }

    #[tokio::test]
    async fn test_mock_swap_functionality() {
        // Test the mock swap implementation used in integration tests

        let swap = MockSwap::new(2.0); // 1 input = 2 output

        let from_asset = FungibleAsset::<BorrowAsset>::nep141("input.near".parse().unwrap());
        let to_asset = FungibleAsset::<BorrowAsset>::nep141("output.near".parse().unwrap());

        // Test quote functionality
        let output_amount = near_sdk::json_types::U128(100);
        let quote_result = swap.quote(&from_asset, &to_asset, output_amount).await;

        assert!(quote_result.is_ok(), "Quote should succeed");
        let input_needed = quote_result.unwrap();
        assert_eq!(
            input_needed.0, 50,
            "Should need 50 input tokens to get 100 output tokens at 2:1 rate"
        );

        // Test swap functionality
        let swap_amount = near_sdk::json_types::U128(25);
        let swap_result = swap.swap(&from_asset, &to_asset, swap_amount).await;

        assert!(swap_result.is_ok(), "Swap should succeed");
        // Mock always returns success, so we just verify it doesn't error
    }

    #[test]
    fn test_asset_specifications() {
        // Test different asset specification formats

        // NEP-141
        let nep141: Result<FungibleAsset<BorrowAsset>, _> = "nep141:token.near".parse();
        assert!(nep141.is_ok(), "NEP-141 parsing should succeed");

        let asset = nep141.unwrap();
        assert_eq!(asset.to_string(), "nep141:token.near");
        assert_eq!(
            asset.contract_id(),
            "token.near".parse::<AccountId>().unwrap()
        );

        // NEP-245
        let nep245: Result<FungibleAsset<BorrowAsset>, _> = "nep245:multi.near:token123".parse();
        assert!(nep245.is_ok(), "NEP-245 parsing should succeed");

        let asset = nep245.unwrap();
        assert_eq!(asset.to_string(), "nep245:multi.near:token123");
        assert_eq!(
            asset.contract_id(),
            "multi.near".parse::<AccountId>().unwrap()
        );

        // Invalid formats should fail
        let invalid: Result<FungibleAsset<BorrowAsset>, _> = "invalid".parse();
        assert!(invalid.is_err(), "Invalid format should fail parsing");
    }

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
