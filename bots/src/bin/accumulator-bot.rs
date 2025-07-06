use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use templar_bots::{
    accumulator::{Args, setup_accumulator},
    near::get_borrows,
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

    let (client, accumulators) = setup_accumulator(&args)?;

    loop {
        for accumulator in &accumulators {
            info!("Accumulator job started for market: {}", accumulator.market);
            let borrows = get_borrows(&client, &accumulator.market, None, None).await?;

            futures::stream::iter(borrows)
                .map(|(account_id, _)| {
                    let accumulator = accumulator.clone();
                    async move { accumulator.accumulate(account_id).await }
                })
                .buffer_unordered(10)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<anyhow::Result<Vec<_>>>()?;
        }

        info!(
            "Accumulation job done, sleeping for {} seconds before next run",
            args.interval
        );
        // Sleep for the specified interval before the next iteration
        sleep(Duration::from_secs(args.interval)).await;
    }
}
