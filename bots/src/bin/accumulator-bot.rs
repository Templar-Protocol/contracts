use std::time::Duration;

use clap::Parser;
use templar_bots::accumulator::{Accumulator, Args};
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

    let accumulators = Accumulator::setup_accumulators(&args)?;

    loop {
        for accumulator in &accumulators {
            accumulator.run_accumulations(args.concurrency).await?;
        }

        info!(
            "Accumulation job done, sleeping for {} seconds before next run",
            args.interval
        );
        // Sleep for the specified interval before the next iteration
        sleep(Duration::from_secs(args.interval)).await;
    }
}
