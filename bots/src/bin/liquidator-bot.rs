use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use futures::{StreamExt, TryStreamExt};
use near_crypto::InMemorySigner;
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{serde_json::json, AccountId};
use templar_bots::{
    liquidator::{Args, Liquidator, LiquidatorError, LiquidatorResult},
    near::{view, RpcResult},
    swap::{RheaSwap, SwapType},
};
use tokio::time::sleep;
use tracing::{info, instrument};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[instrument(skip(client), level = "debug")]
pub async fn list_deployments(
    client: &JsonRpcClient,
    registry: AccountId,
    count: Option<u32>,
    offset: Option<u32>,
) -> RpcResult<Vec<AccountId>> {
    let mut all_deployments = Vec::new();
    let page_size = 500;
    let mut current_offset = 0;

    loop {
        let params = json!({
            "offset": current_offset,
            "count": page_size,
        });

        let page =
            view::<Vec<AccountId>>(client, registry.clone(), "list_deployments", params).await?;

        let fetched = page.len();

        if fetched == 0 {
            break;
        }

        all_deployments.extend(page);
        current_offset += fetched;

        if fetched < page_size {
            break;
        }
    }

    Ok(all_deployments)
}

#[instrument(skip(client), level = "debug")]
pub async fn list_all_deployments(
    client: JsonRpcClient,
    registries: Vec<AccountId>,
    concurrency: usize,
) -> RpcResult<Vec<AccountId>> {
    let all_markets: Vec<AccountId> = futures::stream::iter(registries)
        .map(|registry| {
            let client = client.clone();
            async move { list_deployments(&client, registry, None, None).await }
        })
        .buffer_unordered(concurrency)
        .try_concat()
        .await?;

    Ok(all_markets)
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
    let swap = match args.swap {
        SwapType::RheaSwap => Arc::new(RheaSwap::new(
            args.swap.account_id(args.network),
            client.clone(),
            signer.clone(),
        )),
    };
    let asset = Arc::new(args.asset);

    let registry_refresh_interval = Duration::from_secs(args.registry_refresh_interval);
    let mut next_refresh = Instant::now();
    let mut markets = HashMap::<AccountId, Liquidator<_>>::new();

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
                        // All clones are Arcs so this is cheap
                        client.clone(),
                        signer.clone(),
                        asset.clone(),
                        // This is the only true clone
                        market.clone(),
                        swap.clone(),
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
        // Sleep for the specified interval before the next iteration
        sleep(Duration::from_secs(args.interval)).await;
    }
}
