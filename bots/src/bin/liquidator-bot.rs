use std::time::Duration;

use clap::Parser;
use templar_bots::liquidator::{Args, Liquidator};
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

    let liquidators = Liquidator::setup_liquidators(&args)?;

    loop {
        for liquidator in &liquidators {
            liquidator
                .run_liquidations(args.network.get_oracle_account_id(), args.concurrency)
                .await?;
        }

        info!(
            "Liquidation job done, sleeping for {} seconds before next run",
            args.interval
        );
        // Sleep for the specified interval before the next iteration
        sleep(Duration::from_secs(args.interval)).await;
    }
}
