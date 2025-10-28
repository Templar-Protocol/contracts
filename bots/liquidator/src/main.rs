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
    rpc::{list_all_deployments, Network},
    strategy::PartialLiquidationStrategy,
    swap::{intents::IntentsSwap, rhea::RheaSwap, SwapProviderImpl},
    Liquidator, LiquidatorError, SwapType,
};
use tokio::time::sleep;
use tracing::Instrument;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Check if an error is a rate limit error
fn is_rate_limit_error(error: &LiquidatorError) -> bool {
    let error_msg = error.to_string();
    error_msg.contains("TooManyRequests")
        || error_msg.contains("429")
        || error_msg.contains("rate limit")
}

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
    /// Custom RPC URL (overrides default network RPC)
    #[arg(long, env = "RPC_URL")]
    pub rpc_url: Option<String>,
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
    /// Dry run mode - scan markets and log liquidation opportunities without executing transactions
    #[arg(long, env = "DRY_RUN", default_value_t = false)]
    pub dry_run: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize tracing with enhanced formatting
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_line_number(false)
                .with_file(false),
        )
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,templar_liquidator=debug")),
        )
        .init();

    tracing::info!(network = %args.network, dry_run = args.dry_run, "Starting liquidator bot");
    if args.dry_run {
        tracing::info!(
            "DRY RUN MODE: Will scan and log opportunities without executing liquidations"
        );
    }
    run_bot(args).await;
}

async fn run_bot(args: Args) {
    let rpc_url = args
        .rpc_url
        .as_deref()
        .unwrap_or_else(|| args.network.rpc_url());
    tracing::info!(rpc_url = %rpc_url, "Connecting to RPC");
    let client = JsonRpcClient::connect(rpc_url);
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
            let refresh_span = tracing::debug_span!("registry_refresh");

            let refresh_result: Result<(), LiquidatorError> = async {
                tracing::info!("Refreshing registry deployments");

                let all_markets = match list_all_deployments(
                    client.clone(),
                    args.registries.clone(),
                    args.concurrency,
                )
                .await
                {
                    Ok(markets) => markets,
                    Err(e) => {
                        return Err(LiquidatorError::ListDeploymentsError(e));
                    }
                };

                tracing::info!(
                    market_count = all_markets.len(),
                    markets = ?all_markets,
                    "Found deployments from registries"
                );

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
                            args.dry_run,
                        );
                        (market, liquidator)
                    })
                    .collect();
                Ok(())
            }
            .instrument(refresh_span)
            .await;

            // Handle registry refresh errors gracefully
            match refresh_result {
                Ok(()) => {
                    tracing::info!("Registry refresh completed successfully");
                    next_refresh = Instant::now() + registry_refresh_interval;
                }
                Err(e) => {
                    if is_rate_limit_error(&e) {
                        tracing::error!(
                            error = %e,
                            "Rate limit hit during registry refresh, will retry in 60 seconds"
                        );
                        next_refresh = Instant::now() + Duration::from_secs(60);
                    } else {
                        tracing::error!(
                            error = %e,
                            "Registry refresh failed, will retry in 5 minutes"
                        );
                        next_refresh = Instant::now() + Duration::from_secs(300);
                    }

                    if markets.is_empty() {
                        tracing::warn!("No markets available yet, waiting before retry");
                        sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                }
            }
        }

        let liquidation_span = tracing::debug_span!("liquidation_round");

        // Run liquidations for all markets - don't propagate errors
        async {
            for (i, (market, liquidator)) in markets.iter().enumerate() {
                let market_span = tracing::debug_span!("market", market = %market);

                let result = async {
                    tracing::info!(market = %market, "Scanning market for liquidations");
                    liquidator.run_liquidations(args.concurrency).await
                }
                .instrument(market_span)
                .await;

                // Handle errors gracefully
                match result {
                    Ok(()) => {
                        tracing::info!(market = %market, "Market scan completed successfully");
                    }
                    Err(e) => {
                        if is_rate_limit_error(&e) {
                            tracing::error!(
                                market = %market,
                                error = %e,
                                "Rate limit hit while scanning market, sleeping 60 seconds before continuing"
                            );
                            sleep(Duration::from_secs(60)).await;
                        } else {
                            tracing::error!(
                                market = %market,
                                error = %e,
                                "Failed to scan market, continuing to next market"
                            );
                        }
                    }
                }

                // Add delay between markets to avoid rate limiting (except after last market)
                if i < markets.len() - 1 {
                    let delay_seconds = 5;
                    tracing::debug!(
                        "Waiting {}s before next market to avoid rate limits",
                        delay_seconds
                    );
                    sleep(Duration::from_secs(delay_seconds)).await;
                }
            }
        }
        .instrument(liquidation_span)
        .await;

        tracing::info!(
            interval_seconds = args.interval,
            "Liquidation round completed, sleeping before next run"
        );
        sleep(Duration::from_secs(args.interval)).await;
    }
}
