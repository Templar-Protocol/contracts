use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use templar_liquidator::{
    Liquidator, LiquidatorError, LiquidatorResult, SwapType,
    strategy::PartialLiquidationStrategy,
    swap::{intents::IntentsSwap, rhea::RheaSwap, SwapProviderImpl},
};
use templar_bots_common::{list_all_deployments, Network};
use tokio::time::sleep;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Command-line arguments for the liquidator bot.
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
    pub signer_key: near_crypto::SecretKey,
    /// Signer `AccountId`.
    #[arg(short, long, env = "SIGNER_ACCOUNT_ID")]
    pub signer_account: AccountId,
    /// Asset specification (NEP-141 or NEP-245) to liquidate with
    #[arg(short, long, env = "ASSET_SPEC")]
    pub asset: templar_common::asset::FungibleAsset<templar_common::asset::BorrowAsset>,
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
    /// Concurrency for liquidations
    #[arg(short, long, env = "CONCURRENCY", default_value_t = 10)]
    pub concurrency: usize,
    /// Partial liquidation percentage (1-100)
    #[arg(long, env = "PARTIAL_PERCENTAGE", default_value_t = 50)]
    pub partial_percentage: u8,
    /// Minimum profit margin in basis points
    #[arg(long, env = "MIN_PROFIT_BPS", default_value_t = 50)]
    pub min_profit_bps: u32,
    /// Maximum gas cost percentage
    #[arg(long, env = "MAX_GAS_PERCENTAGE", default_value_t = 10)]
    pub max_gas_percentage: u8,
}

#[tokio::main]
async fn main() -> LiquidatorResult {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let client = JsonRpcClient::connect(args.network.rpc_url());
    let signer = Arc::new(InMemorySigner::from_secret_key(
        args.signer_account.clone(),
        args.signer_key.clone(),
    ));

    // Create swap provider based on CLI argument
    let swap_provider = match args.swap {
        SwapType::RheaSwap => {
            let rhea = RheaSwap::new(
                args.swap.account_id(args.network),
                client.clone(),
                signer.clone(),
            );
            SwapProviderImpl::rhea(rhea)
        }
        SwapType::NearIntents => {
            let intents = IntentsSwap::new(client.clone(), signer.clone(), args.network);
            SwapProviderImpl::intents(intents)
        }
    };

    // Create liquidation strategy
    let strategy = Box::new(PartialLiquidationStrategy::new(
        args.partial_percentage,
        args.min_profit_bps,
        args.max_gas_percentage,
    ));

    let asset = Arc::new(args.asset);

    let registry_refresh_interval = Duration::from_secs(args.registry_refresh_interval);
    let mut next_refresh = Instant::now();
    let mut markets = HashMap::<AccountId, Liquidator>::new();

    loop {
        if Instant::now() >= next_refresh {
            info!("Refreshing registry deployments");
            let all_markets =
                list_all_deployments(client.clone(), args.registries.clone(), args.concurrency)
                    .await
                    .map_err(LiquidatorError::ListDeploymentsError)?;
            info!("Found {} deployments", all_markets.len());

            markets = all_markets
                .into_iter()
                .map(|market| {
                    let liquidator = Liquidator::new(
                        client.clone(),
                        signer.clone(),
                        asset.clone(),
                        market.clone(),
                        swap_provider.clone(),
                        strategy.clone(),
                        args.timeout,
                    );
                    (market, liquidator)
                })
                .collect();
            next_refresh = Instant::now() + registry_refresh_interval;
        }

        for (market, liquidator) in &markets {
            info!("Running liquidations for market: {}", market);
            liquidator.run_liquidations(args.concurrency).await?;
        }

        info!(
            "Liquidation job done, sleeping for {} seconds before next run",
            args.interval
        );
        sleep(Duration::from_secs(args.interval)).await;
    }
}
