use std::sync::Arc;

use clap::Parser;
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{
    AccountId, BorshStorageKey,
    json_types::U128,
    near,
    serde_json::{self, json},
};
use templar_common::{
    borrow::{BorrowPosition, BorrowStatus},
    market::{DepositMsg, LiquidateMsg},
    oracle::pyth::OracleResponse,
};
use tracing::{error, info, instrument};

use crate::{
    Network,
    near::{ft_transfer_call, get_borrow_status},
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

    /// Signer key to use for signing transactions.
    #[arg(short, long, env = "SIGNER_KEY")]
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

    /// Concurency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,

    /// Interval between liquidation attempts
    #[arg(short, long, env = "INTERVAL", default_value_t = 600)]
    pub interval: u64,
}

pub struct Liquidator {
    client: JsonRpcClient,
    signer: InMemorySigner,
    asset: AccountId,
    pub market: AccountId,
    timeout: u64,
}

impl Liquidator {
    #[must_use]
    pub fn new(
        client: JsonRpcClient,
        signer: InMemorySigner,
        asset: AccountId,
        market: AccountId,
        timeout: u64,
    ) -> Self {
        Self {
            client,
            signer,
            asset,
            market,
            timeout,
        }
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn try_liquidate(
        &self,
        borrow: AccountId,
        position: BorrowPosition,
        oracle_response: OracleResponse,
    ) -> anyhow::Result<()> {
        let Some(status) = get_borrow_status(
            &self.client,
            self.market.clone(),
            borrow.clone(),
            &oracle_response,
        )
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

        let liquidation_amount = self.liquidation_amount(&position, &oracle_response)?;

        #[allow(clippy::unwrap_used, reason = "We know the serialization will succeed")]
        let msg = serde_json::to_string(&DepositMsg::Liquidate(LiquidateMsg {
            account_id: borrow.clone(),
        }))
        .unwrap();

        match ft_transfer_call(
            &self.client,
            &self.signer,
            self.asset.clone(),
            json!({
                "receiver_id": self.market,
                "amount": liquidation_amount,
                "msg": msg,
            }),
            self.timeout,
        )
        .await
        {
            Ok(_) => {
                info!("Liquidation successful");
            }
            Err(e) => {
                error!("Liquidation failed: {e}");
            }
        }

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    #[allow(
        clippy::used_underscore_binding,
        reason = "Still need to implement this"
    )]
    fn liquidation_amount(
        &self,
        position: &BorrowPosition,
        _oracle_response: &OracleResponse,
    ) -> anyhow::Result<U128> {
        // TODO: Calculate optimal liquidation amount
        // For purposes of this example implementation we will just use the total borrow amount
        // Costs to take into account here are:
        //  - Liquidation spread
        //  - Gas fees
        //  - Price impact
        //  - Slippage
        //  - Possible flash loan fees
        // All of this would be used in calculating both the optimal liquidation amount and wether to
        // perform full or partial liquidation
        Ok(position.get_total_borrow_asset_liability().into())
    }
}

#[instrument(level = "debug")]
pub fn setup_liquidator(args: &Args) -> anyhow::Result<(JsonRpcClient, Vec<Arc<Liquidator>>)> {
    let client = JsonRpcClient::connect(args.network.get_rpc_url());
    let signer =
        InMemorySigner::from_secret_key(args.signer_account.clone(), args.signer_key.clone());
    let asset = args.asset.clone();

    Ok((
        client.clone(),
        args.markets
            .iter()
            .map(|market| {
                Arc::new(Liquidator::new(
                    client.clone(),
                    signer.clone(),
                    asset.clone(),
                    market.clone(),
                    args.timeout,
                ))
            })
            .collect::<Vec<_>>(),
    ))
}
