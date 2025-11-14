use std::{collections::HashMap, sync::Arc, time::Duration};

use clap::Parser;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use templar_accumulator::{rpc::list_all_deployments, Accumulator, Args};
use tracing::{error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!("Starting accumulator bot with args: {args}");
    let client = JsonRpcClient::connect(args.network.rpc_url());
    let signer = Arc::new(InMemorySigner::from_secret_key(
        args.signer_account.clone(),
        args.signer_key.clone(),
    ));

    let mut refresh_ticker =
        tokio::time::interval(Duration::from_secs(args.registry_refresh_interval));
    let mut accumulate_ticker = tokio::time::interval(Duration::from_secs(args.interval));
    let mut static_accumulate_ticker =
        tokio::time::interval(Duration::from_secs(args.static_interval));
    let mut accumulators =
        list_all_deployments(client.clone(), args.registries.clone(), args.concurrency)
            .await?
            .into_iter()
            .map(|market| {
                (
                    market.clone(),
                    Accumulator::new(client.clone(), signer.clone(), market, args.timeout),
                )
            })
            .collect::<HashMap<_, _>>();

    loop {
        tokio::select! {
            _ = refresh_ticker.tick() => {
                info!("Refreshing registry deployments");
                let Ok(all_markets) =
                    list_all_deployments(client.clone(), args.registries.clone(), args.concurrency)
                        .await else {
                    error!("Failed to list deployments, keeping existing ones");
                    continue;
                };
                info!("Found {} deployments", all_markets.len());
                for market in all_markets {
                    accumulators.entry(market.clone()).or_insert_with(|| {
                        Accumulator::new(client.clone(), signer.clone(), market, args.timeout)
                    });
                }
            }
            _ = accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running accumulation for market: {market}");
                    accumulator.run_borrow_accumulations(args.concurrency).await?;
                }

                info!("Accumulation job done");
            }
            _ = static_accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running static accumulation for market: {market}");
                    accumulator.run_static_accumulations(args.concurrency).await?;
                }

                info!("Static accumulation job done");
            }
        }
    }
}
