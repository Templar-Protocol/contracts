use std::time::Duration;

use clap::Parser;
use templar_bots::liquidator::{setup_liquidators, Args, LiquidatorResult};
use tokio::time::sleep;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> LiquidatorResult {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let liquidators = setup_liquidators(&args)?;

    loop {
        for liquidator in &liquidators {
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
