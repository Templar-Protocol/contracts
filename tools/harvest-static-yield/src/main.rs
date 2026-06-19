use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    str::FromStr,
};

use anyhow::Context;
use clap::Parser;
use near_account_id::AccountId;
use near_api::{NetworkConfig, SecretKey};
use templar_common::asset::{BorrowAsset, BorrowAssetAmount, FungibleAsset};
use templar_gateway_client::Client;
use templar_gateway_methods_spec::{contract, ft, market, registry, storage};
use templar_gateway_types::{common::Pagination, Market, MarketVersion, NearToken, U128};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Debug)]
enum Network {
    Mainnet,
    Testnet,
}

impl Network {
    pub fn rpc_url(&self) -> &str {
        match self {
            Self::Mainnet => "https://rpc.mainnet.near.org",
            Self::Testnet => "https://rpc.testnet.near.org",
        }
    }
}

impl FromStr for Network {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "mainnet" => Ok(Self::Mainnet),
            "testnet" => Ok(Self::Testnet),
            _ => Err("expected \"mainnet\" or \"testnet\""),
        }
    }
}

impl Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
        }
        .fmt(f)
    }
}

#[derive(Debug, Clone, Parser)]
struct Cli {
    /// Registries for which to harvest static yield on every market.
    #[arg(short, long, env = "REGISTRY_ID", value_delimiter = ',')]
    pub registry_id: Vec<AccountId>,
    /// Markets on which to harvest static yield.
    #[arg(short, long, env = "MARKET_ID", value_delimiter = ',')]
    pub market_id: Vec<AccountId>,
    /// Network to connect to.
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    pub network: Network,
    /// Specify a custom RPC URL.
    #[arg(long, env = "RPC_URL")]
    pub rpc_url: Option<String>,
    /// Account ID to harvest static yield for.
    #[arg(short, long, env = "ACCOUNT_ID")]
    pub account_id: AccountId,
    /// Signing key for the account that is harvesting static yield.
    #[arg(short, long, env = "SECRET_KEY")]
    pub secret_key: SecretKey,
    /// Account ID to forward the yield to after harvesting (e.g. another treasury account).
    #[arg(long, env = "RECEIVER_ID")]
    pub receiver_id: Option<AccountId>,
}

async fn market_version(client: &Client, market_id: AccountId) -> anyhow::Result<MarketVersion> {
    let version = client
        .read(contract::GetVersion {
            contract_id: market_id.clone(),
        })
        .await?;
    version.parsed_as::<Market>().with_context(|| {
        format!(
            "market {market_id} reported an unparseable version \"{}\"",
            version.version_string
        )
    })
}

#[allow(clippy::too_many_lines)]
#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Cli::parse();

    tracing::info!(account_id = %args.account_id, "Harvesting static yield");

    let rpc_url = args
        .rpc_url
        .as_deref()
        .unwrap_or_else(|| args.network.rpc_url());

    tracing::info!(network = %args.network, rpc_url = %rpc_url, "Connecting to RPC");

    let network = NetworkConfig::from_rpc_url(
        &args.network.to_string(),
        rpc_url.parse().context("invalid RPC URL")?,
    );

    let client = Client::builder(network)
        .secret_key(args.account_id.clone(), args.secret_key.clone())?
        .build()?;

    let mut markets: HashSet<AccountId> = args.market_id.iter().cloned().collect();

    for registry in args.registry_id {
        tracing::info!(%registry, "Loading markets from registry");
        match client
            .read(registry::ListDeployments {
                registry_id: registry.clone(),
                args: Pagination::default(),
            })
            .await
        {
            Ok(result) => markets.extend(result.account_ids),
            Err(error) => {
                tracing::error!(%registry, %error, "Failed to list deployments on registry");
            }
        }
    }

    if markets.is_empty() {
        tracing::error!("No markets specified");
        std::process::exit(1);
    }

    let mut accumulated_assets: HashMap<FungibleAsset<BorrowAsset>, BorrowAssetAmount> =
        HashMap::new();

    for market_id in markets {
        tracing::info!(%market_id, "Processing market");

        let (version, configuration) = tokio::join!(
            market_version(&client, market_id.clone()),
            client.read(market::GetConfiguration {
                market_id: market_id.clone(),
            }),
        );
        let version = match version {
            Ok(version) => version,
            Err(error) => {
                tracing::warn!(%error, "Failed to fetch market version");
                continue;
            }
        };
        let configuration = match configuration {
            Ok(configuration) => configuration,
            Err(error) => {
                tracing::warn!(%error, "Failed to fetch market configuration");
                continue;
            }
        };

        let asset_contract = configuration.borrow_asset.contract_id().to_owned();
        tracing::info!(%market_id, "Checking storage requirements");
        if let Ok(bounds) = client
            .read(storage::GetBalanceBounds {
                contract_id: asset_contract.clone(),
            })
            .await
        {
            let balance = match client
                .read(storage::GetBalanceOf {
                    contract_id: asset_contract.clone(),
                    account_id: args.account_id.clone(),
                })
                .await
            {
                Ok(result) => result.balance,
                Err(error) => {
                    tracing::error!(%asset_contract, account_id = %args.account_id, %error, "Failed to fetch storage balance from asset contract that has a balance requirement");
                    std::process::exit(1);
                }
            };

            let storage_balance_total = balance.map_or(NearToken::from_yoctonear(0), |b| b.total);

            if storage_balance_total < bounds.bounds.min {
                tracing::error!(%market_id, %asset_contract, min = %bounds.bounds.min, %storage_balance_total, "Insufficient storage deposit on asset contract");
                continue;
            }
        }

        if version.requires_static_yield_accumulation() {
            tracing::info!(%market_id, %version, "Running static yield accumulation");
            if let Err(error) = client
                .execute(market::AccumulateStaticYield {
                    market_id: market_id.clone(),
                    account_id: None,
                    snapshot_limit: None,
                })
                .await
            {
                tracing::error!(%market_id, %version, %error, "Failed to run static yield accumulation");
                continue;
            };
        }

        let yield_amount = match client
            .read(market::GetStaticYield {
                market_id: market_id.clone(),
                account_id: args.account_id.clone(),
            })
            .await
        {
            Ok(result) => result.borrow_asset_total,
            Err(error) => {
                tracing::error!(%market_id, %error, "Failed to fetch static yield amount");
                continue;
            }
        };

        if yield_amount.is_zero() {
            tracing::info!(%market_id, "No yield accumulated for market, skipping");
            continue;
        }

        tracing::info!(%market_id, %yield_amount, "Withdrawing yield");
        let transaction_hash = match client
            .execute(market::WithdrawStaticYield {
                market_id: market_id.clone(),
                amount: None,
            })
            .await
        {
            Ok(transaction_hash) => transaction_hash,
            Err(error) => {
                tracing::error!(%market_id, %error, "Failed to withdraw static yield");
                continue;
            }
        };

        tracing::info!(%market_id, %yield_amount, %transaction_hash, "Successfully withdrew yield");

        *accumulated_assets
            .entry(configuration.borrow_asset)
            .or_insert(0.into()) += yield_amount;
    }

    if accumulated_assets.is_empty() {
        tracing::info!("No assets accumulated");
        return Ok(());
    }

    for (asset, amount) in &accumulated_assets {
        tracing::info!(%asset, %amount, "Withdrawn");
    }

    let Some(receiver_id) = args.receiver_id else {
        return Ok(());
    };

    tracing::info!(%receiver_id, "Yield receiver");

    let mut transfer_failures = 0;
    for (asset, amount) in accumulated_assets {
        tracing::info!(%asset, %receiver_id, %amount, "Sending yield");
        match client
            .execute(ft::Transfer {
                contract_id: asset.contract_id().to_owned(),
                receiver_id: receiver_id.clone(),
                amount: U128(u128::from(amount)),
                memo: None,
            })
            .await
        {
            Ok(transaction_hash) => {
                tracing::info!(%asset, %receiver_id, %amount, %transaction_hash, "Transferred to receiver");
            }
            Err(error) => {
                tracing::error!(%asset, %receiver_id, %amount, %error, "Failed to send tokens to receiver");
                transfer_failures += 1;
            }
        };
    }

    if transfer_failures > 0 {
        tracing::error!(transfer_failures, "Some transfers failed");
        std::process::exit(1);
    } else {
        tracing::info!("All transfers completed successfully.");
    }

    Ok(())
}
