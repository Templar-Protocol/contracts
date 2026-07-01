use std::{collections::HashMap, future::Future, time::Duration};

use anyhow::Context;
use clap::Parser;
use templar_accumulator::{list_all_deployments, Accumulator, Args};
use templar_gateway_client::{NetworkConfigBuilder, SigningClient};
use tracing::{error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!("Starting accumulator bot with args:\n{args}");
    run_service(args, std::future::pending()).await
}

async fn run_service_with_client(
    args: Args,
    client: SigningClient,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // The accumulator only discovers markets via registries, so an empty set
    // means the daemon would run forever doing nothing. Fail fast.
    anyhow::ensure!(
        !args.registries.is_empty(),
        "at least one registry must be configured (REGISTRIES_ACCOUNT_IDS / --registries)"
    );

    // Zero would panic `tokio::time::interval` / break `buffer_unordered`; fail
    // fast rather than crash mid-run on a bad env/CLI override.
    anyhow::ensure!(
        args.interval > 0
            && args.static_interval > 0
            && args.registry_refresh_interval > 0
            && args.concurrency > 0,
        "interval, static_interval, registry_refresh_interval, and concurrency must all be > 0"
    );

    let registries = args.registries.clone();
    let concurrency = args.concurrency;

    let mut refresh_ticker =
        tokio::time::interval(Duration::from_secs(args.registry_refresh_interval));
    let mut accumulate_ticker = tokio::time::interval(Duration::from_secs(args.interval));
    let mut static_accumulate_ticker =
        tokio::time::interval(Duration::from_secs(args.static_interval));
    let mut accumulators = list_all_deployments(&client, registries.clone(), concurrency)
        .await?
        .into_iter()
        .map(|market| (market.clone(), Accumulator::new(client.clone(), market)))
        .collect::<HashMap<_, _>>();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("Shutdown signal received, stopping accumulator bot");
                break;
            }
            _ = refresh_ticker.tick() => {
                info!("Refreshing registry deployments");
                let Ok(all_markets) =
                    list_all_deployments(&client, registries.clone(), concurrency).await else {
                    error!("Failed to list deployments, keeping existing ones");
                    continue;
                };
                info!("Found {} deployments", all_markets.len());
                for market in all_markets {
                    accumulators.entry(market.clone()).or_insert_with(|| {
                        Accumulator::new(client.clone(), market)
                    });
                }
            }
            _ = accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running accumulation for market: {market}");
                    if let Err(err) = accumulator.run_borrow_accumulations(concurrency).await {
                        error!("Borrow accumulation failed for market {market}: {err}");
                    }
                }

                info!("Accumulation job done");
            }
            _ = static_accumulate_ticker.tick() => {
                for (market, accumulator) in &accumulators {
                    info!("Running static accumulation for market: {market}");
                    if let Err(err) = accumulator.run_static_accumulations(concurrency).await {
                        error!("Static accumulation failed for market {market}: {err}");
                    }
                }

                info!("Static accumulation job done");
            }
        }
    }

    Ok(())
}

async fn run_service(
    args: Args,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // Don't log the resolved RPC URL: an embedded `apiKey` makes it secret-bearing.
    info!(
        network = %args.network,
        custom_rpc = args.rpc_url.is_some(),
        "Connecting to RPC"
    );

    let network = NetworkConfigBuilder::new(args.network)
        .rpc_url(args.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(args.rpc_api_key.clone())
        .build();

    let client = SigningClient::connect(
        network,
        args.signer_account.clone(),
        args.signer_key.clone(),
    )?;

    run_service_with_client(args, client, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{KeyType, SecretKey};
    use std::env;

    use near_account_id::AccountId;

    #[test]
    fn registries_env_is_space_delimited() {
        let sk = SecretKey::from_random(KeyType::ED25519);
        let original_regs = env::var("REGISTRIES_ACCOUNT_IDS").ok();
        let original_signer = env::var("SIGNER_ACCOUNT_ID").ok();
        let original_key = env::var("SIGNER_KEY").ok();

        env::set_var("REGISTRIES_ACCOUNT_IDS", "one.testnet two.testnet");
        env::set_var("SIGNER_ACCOUNT_ID", "signer.testnet");
        env::set_var("SIGNER_KEY", sk.to_string());

        let args = Args::parse_from(["accumulator"]);
        let expected: Vec<AccountId> = vec![
            "one.testnet".parse().unwrap(),
            "two.testnet".parse().unwrap(),
        ];

        assert_eq!(args.registries, expected);

        if let Some(val) = original_regs {
            env::set_var("REGISTRIES_ACCOUNT_IDS", val);
        } else {
            env::remove_var("REGISTRIES_ACCOUNT_IDS");
        }
        if let Some(val) = original_signer {
            env::set_var("SIGNER_ACCOUNT_ID", val);
        } else {
            env::remove_var("SIGNER_ACCOUNT_ID");
        }
        if let Some(val) = original_key {
            env::set_var("SIGNER_KEY", val);
        } else {
            env::remove_var("SIGNER_KEY");
        }
    }
}
