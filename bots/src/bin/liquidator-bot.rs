use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use templar_bots::{
    liquidator::{Args, setup_liquidator},
    near::{get_borrows, get_configuration, get_oracle_prices},
};
use tokio::time::sleep;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let (client, liquidators) = setup_liquidator(&args)?;

    loop {
        for liquidator in &liquidators {
            info!("Liquidation job started for market: {}", liquidator.market);
            let borrows = get_borrows(&client, &liquidator.market, None, None).await?;
            // let borrows = get_borrows(&client, &liquidator.market).await?;
            let configuration = get_configuration(&client, liquidator.market.clone()).await?;

            let borrow_id = configuration.balance_oracle.borrow_asset_price_id;
            let collateral_id = configuration.balance_oracle.collateral_asset_price_id;
            let age = configuration.balance_oracle.price_maximum_age_s;

            let oracle_response = get_oracle_prices(
                &client,
                args.network.get_contract(),
                &[borrow_id, collateral_id],
                age,
            )
            .await?;

            futures::stream::iter(borrows)
                .map(|(borrow, position)| {
                    let liquidator = liquidator.clone();
                    let oracle_response = oracle_response.clone();
                    async move {
                        liquidator
                            .try_liquidate(borrow, position, oracle_response)
                            .await
                    }
                })
                .buffer_unordered(args.concurrency)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<anyhow::Result<Vec<_>>>()?;
        }

        info!(
            "Liquidation job done, sleeping for {} seconds before next run",
            args.interval
        );
        // Sleep for the specified interval before the next iteration
        sleep(Duration::from_secs(args.interval)).await;
    }
}
