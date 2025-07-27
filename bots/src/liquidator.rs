use std::collections::HashMap;

use clap::Parser;
use futures::StreamExt;
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{Transaction, TransactionV0},
};
use near_sdk::{
    AccountId, BorshStorageKey,
    json_types::U128,
    near,
    serde_json::{self, json},
};
use templar_common::{
    borrow::{BorrowPosition, BorrowStatus},
    market::{DepositMsg, LiquidateMsg, MarketConfiguration},
    oracle::pyth::{OracleResponse, PriceIdentifier},
};
use tracing::{error, info, instrument};

use crate::{
    BorrowPositions, DEFAULT_GAS, Network, ONE_YOCTO_NEAR,
    near::{get_access_key_data, send_tx, serialize_and_encode, view},
    swap::{RheaSwap, Swap, SwapType},
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum MarketStorageKey {
    Market,
}

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum InnerStorageKey {
    SupplyPositions,
    BorrowPositions,
    FinalizedSnapshots,
    WithdrawalQueue,
    StaticYield,
}

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Market to run liquidations for
    #[arg(short, long, env = "MARKET_ACCOUNT_ID")]
    pub markets: Vec<AccountId>,
    /// Swap to use for liquidations
    #[arg(long, env = "SWAP_TYPE")]
    pub swap: SwapType,
    /// Signer key to use for signing transactions.
    #[arg(short = 'k', long, env = "SIGNER_KEY")]
    pub signer_key: SecretKey,
    /// Signer `AccountId`.
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Asset to liquidate
    #[arg(short, long, env = "ASSET_ACCOUNT_ID")]
    pub asset: AccountId,
    /// Network to run liquidations on
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Timeout for transactions
    #[arg(short, long, env = "TIMEOUT", default_value_t = 60)]
    pub timeout: u64,
    /// Interval between liquidation attempts
    #[arg(short, long, env = "INTERVAL", default_value_t = 600)]
    pub interval: u64,
    /// Concurency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,
}

pub struct Liquidator<S: Swap> {
    client: JsonRpcClient,
    signer: InMemorySigner,
    asset: AccountId,
    pub market: AccountId,
    timeout: u64,
    swap: S,
}

impl<S: Swap> Liquidator<S> {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: InMemorySigner,
        asset: AccountId,
        market: AccountId,
        swap: S,
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

    #[instrument(skip(self), level = "debug")]
    async fn get_borrow_status(
        &self,
        borrow: AccountId,
        oracle_response: &OracleResponse,
    ) -> anyhow::Result<Option<BorrowStatus>> {
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
    }

    fn create_transfer_tx(
        &self,
        borrow: &AccountId,
        liquidation_amount: U128,
        nonce: u64,
        block_hash: CryptoHash,
    ) -> Transaction {
        #[allow(clippy::unwrap_used, reason = "We know the serialization will succeed")]
        let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
            account_id: borrow.clone(),
        }))
        .unwrap();

        Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: self.asset.clone(),
            block_hash,
            signer_id: self.signer.account_id.clone(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "ft_transfer_call".to_string(),
                args: serialize_and_encode(json!({
                    "receiver_id": self.market,
                    "amount": liquidation_amount,
                    "msg": msg,
                })),
                gas: DEFAULT_GAS,
                deposit: ONE_YOCTO_NEAR,
            }))],
        })
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn try_liquidate(
        &self,
        borrow: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
        configuration: MarketConfiguration,
    ) -> anyhow::Result<()> {
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

        let Some(borrow_asset) = configuration.borrow_asset.clone().into_nep141() else {
            unreachable!("Only NEP-141 and NEP-245 assets are supported");
        };

        let collateral_asset = configuration.collateral_asset.contract_id();

        let (swap_amount, liquidation_amount) = self
            .liquidation_amount(&position, &oracle_response, configuration)
            .await?;

        if self.asset != borrow_asset {
            match self
                .swap
                .swap(&self.asset, &borrow_asset, swap_amount)
                .await
            {
                Ok(_) => {
                    info!("Swap successful");
                }
                Err(e) => {
                    error!("Swap failed: {e}");
                    return Err(e);
                }
            };
        }

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let transfer_call_tx =
            self.create_transfer_tx(&borrow, liquidation_amount, nonce, block_hash);

        match send_tx(&self.client, &self.signer, self.timeout, transfer_call_tx).await {
            Ok(_) => {
                info!("Liquidation successful");
            }
            Err(e) => {
                error!("Liquidation failed: {e}");
                return Err(e);
            }
        }

        if self.asset == collateral_asset {
            match self
                .swap
                .swap(
                    &collateral_asset,
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
        configuration: MarketConfiguration,
    ) -> anyhow::Result<(U128, U128)> {
        // TODO: Calculate optimal liquidation amount
        // For purposes of this example implementation we will just use the minimum acceptable
        // liquidation amount.
        // Costs to take into account here are:
        //  - Gas fees
        //  - Price impact
        //  - Slippage
        // All of this would be used in calculating both the optimal liquidation amount and wether to
        // perform full or partial liquidation
        let borrow_asset = &configuration.borrow_asset;
        let collateral_asset = &configuration.collateral_asset;
        let price_pair = configuration
            .price_oracle_configuration
            .create_price_pair(oracle_response)
            .map_err(|e| anyhow::anyhow!("Failed to create price pair: {e}"))?;
        let min_liquidation_amount = configuration
            .minimum_acceptable_liquidation_amount(position.collateral_asset_deposit, &price_pair)
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to calculate minimum acceptable liquidation amount")
            })?;
        // Here we would check a quote for the swap to ensure desired profit margin is met
        let quote_to_liquidate = self
            .swap
            .quote(
                &self.asset,
                &borrow_asset
                    .clone()
                    .into_nep141()
                    .ok_or_else(|| anyhow::anyhow!("Only NEP-141 borrow assets supported"))?,
                min_liquidation_amount.into(),
            )
            .await?;
        let _quote_after_liquidate = self
            .swap
            .quote(
                // TODO: Enable multitoken swaps
                &collateral_asset.contract_id(),
                &self.asset,
                position.collateral_asset_deposit.into(),
            )
            .await?;
        Ok((quote_to_liquidate, min_liquidation_amount.into()))
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_configuration(&self) -> anyhow::Result<MarketConfiguration> {
        view(
            &self.client,
            self.market.clone(),
            "get_configuration",
            json!({}),
        )
        .await
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> anyhow::Result<OracleResponse> {
        view(
            &self.client,
            oracle,
            "list_ema_prices_no_older_than",
            json!({ "price_ids": price_ids, "age": age }),
        )
        .await
    }

    #[instrument(skip(self), level = "debug")]
    async fn get_borrows(&self) -> anyhow::Result<BorrowPositions> {
        let mut all_positions: BorrowPositions = HashMap::new();

        let page_size = 100;
        let mut current_offset = 0;
        let mut params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        while let Ok(page) = view::<BorrowPositions>(
            &self.client,
            self.market.clone(),
            "list_borrow_positions",
            params.clone(),
        )
        .await
        {
            let fetched = page.len();
            all_positions.extend(page);
            current_offset += page_size;
            params["offset"] = current_offset.into();

            if fetched < page_size {
                break;
            }
        }

        Ok(all_positions)
    }

    #[instrument(skip(self), level = "info")]
    pub async fn run_liquidations(&self, concurrency: usize) -> anyhow::Result<()> {
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

        futures::stream::iter(self.get_borrows().await?)
            .map(|(borrow, position)| {
                let oracle_response = oracle_response.clone();
                let configuration = configuration.clone();
                async move {
                    self.try_liquidate(borrow, position, oracle_response, configuration)
                        .await
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(())
    }
}

#[instrument(level = "debug")]
pub fn setup_liquidators(args: &Args) -> anyhow::Result<Vec<Liquidator<impl Swap>>> {
    let client = JsonRpcClient::connect(args.network.get_rpc_url());
    let signer =
        InMemorySigner::from_secret_key(args.signer_account.clone(), args.signer_key.clone());
    let asset = args.asset.clone();
    let swap = match args.swap {
        SwapType::RheaSwap => RheaSwap::new(
            args.swap.get_account_id(args.network),
            client.clone(),
            signer.clone(),
        ),
    };

    Ok(args
        .markets
        .iter()
        .map(|market| {
            Liquidator::new(
                client.clone(),
                signer.clone(),
                asset.clone(),
                market.clone(),
                swap.clone(),
                args.timeout,
            )
        })
        .collect::<Vec<_>>())
}
