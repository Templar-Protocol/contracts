use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    str::FromStr,
};

use clap::Parser;
use near_contract_standards::{
    contract_metadata::ContractSourceMetadata,
    storage_management::{StorageBalance, StorageBalanceBounds},
};
use near_crypto::{InMemorySigner, SecretKey};
use near_fetch::signer::{ExposeAccountId, SignerExt};
use near_primitives::hash::CryptoHash;
use near_sdk::{serde_json::json, AccountId, NearToken};
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount, FungibleAsset},
    market::MarketConfiguration,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use harvest_static_yield::market_version::MarketVersion;

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

#[allow(clippy::too_many_lines)]
#[tokio::main]
pub async fn main() {
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

    let near = near_fetch::Client::new(rpc_url);

    let signer = InMemorySigner::from_secret_key(args.account_id.clone(), args.secret_key.clone());

    let mut markets: HashSet<AccountId> = args.market_id.iter().cloned().collect();

    for registry in args.registry_id {
        tracing::info!(%registry, "Loading markets from registry");
        let deployments = match near
            .view(&registry, "list_deployments")
            .args_json(json!({}))
            .await
            .and_then(|r| r.json::<Vec<AccountId>>())
        {
            Ok(d) => d,
            Err(error) => {
                tracing::error!(%registry, %error, "Failed to list deployments on registry");
                continue;
            }
        };

        markets.extend(deployments);
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
            get_market_version(&near, &market_id),
            get_market_configuration(&near, &market_id),
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
        if let Ok(storage_balance_bounds) = near
            .view(&asset_contract, "storage_balance_bounds")
            .args_json(json!({}))
            .await
            .and_then(|r| r.json::<StorageBalanceBounds>())
        {
            let storage_balance = match near
                .view(&asset_contract, "storage_balance_of")
                .args_json(json!({ "account_id": &args.account_id }))
                .await
                .and_then(|r| r.json::<Option<StorageBalance>>())
            {
                Ok(s) => s,
                Err(error) => {
                    tracing::error!(%asset_contract, account_id = %args.account_id, %error, "Failed to fetch storage balance from asset contract that has a balance requirement");
                    std::process::exit(1);
                }
            };

            let storage_balance_total =
                storage_balance.map_or(NearToken::from_near(0), |b| b.total);

            if storage_balance_total < storage_balance_bounds.min {
                tracing::error!(%market_id, %asset_contract, %storage_balance_bounds.min, %storage_balance_total, "Insufficient storage deposit on asset contract");
                continue;
            }
        }

        if version.requires_static_yield_accumulation() {
            tracing::info!(%market_id, %version, "Running static yield accumulation");
            if let Err(error) = accumulate_static_yield(&near, &signer, &market_id).await {
                tracing::error!(%market_id, %version, %error, "Failed to run static yield accumulation");
                continue;
            };
        }

        let yield_amount = match get_static_yield(&near, &market_id, signer.account_id()).await {
            Ok(static_yield) => static_yield,
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
        let transaction_hash = match withdraw_static_yield(&near, &signer, &market_id).await {
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
        return;
    }

    for (asset, amount) in &accumulated_assets {
        tracing::info!(%asset, %amount, "Withdrawn");
    }

    let Some(receiver_id) = args.receiver_id else {
        return;
    };

    tracing::info!(%receiver_id, "Yield receiver");

    let mut transfer_failures = 0;
    for (asset, amount) in accumulated_assets {
        let action = asset.transfer_action(&receiver_id, amount);
        tracing::info!(%asset, %receiver_id, %amount, "Sending yield");
        match near
            .send_tx(
                &signer,
                &asset.contract_id().to_owned(),
                vec![action.into()],
            )
            .await
        {
            Ok(result) => {
                tracing::info!(%asset, %receiver_id, %amount, transaction_hash = %result.transaction.hash, "Transferred to receiver");
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
}

#[tracing::instrument(skip(near, signer))]
pub async fn withdraw_static_yield(
    near: &near_fetch::Client,
    signer: &impl SignerExt,
    market_id: &AccountId,
) -> anyhow::Result<CryptoHash> {
    let result = near
        .call(signer, market_id, "withdraw_static_yield")
        .args_json(json!({}))
        .max_gas()
        .transact()
        .await?;
    Ok(result.details.transaction.hash)
}

#[tracing::instrument(skip(near, signer))]
pub async fn accumulate_static_yield(
    near: &near_fetch::Client,
    signer: &impl SignerExt,
    market_id: &AccountId,
) -> anyhow::Result<()> {
    near.call(signer, market_id, "accumulate_static_yield")
        .args_json(json!({}))
        .max_gas()
        .transact()
        .await?
        .raw_bytes()?;
    Ok(())
}

#[tracing::instrument(skip(near))]
pub async fn get_static_yield(
    near: &near_fetch::Client,
    market_id: &AccountId,
    account_id: &AccountId,
) -> anyhow::Result<BorrowAssetAmount> {
    Ok(near
        .view(market_id, "get_static_yield")
        .args_json(json!({ "account_id": account_id }))
        .await?
        .json::<GetStaticYield>()?
        .borrow_asset_total())
}

#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde", untagged)]
enum GetStaticYield {
    Split {
        #[allow(unused)]
        collateral_asset: CollateralAssetAmount,
        borrow_asset: BorrowAssetAmount,
    },
    Accumulator(Accumulator<BorrowAsset>),
}

impl GetStaticYield {
    fn borrow_asset_total(&self) -> BorrowAssetAmount {
        match self {
            GetStaticYield::Split { borrow_asset, .. } => *borrow_asset,
            GetStaticYield::Accumulator(accumulator) => accumulator.get_total(),
        }
    }
}

#[tracing::instrument(skip(near))]
pub async fn get_market_version(
    near: &near_fetch::Client,
    market_id: &AccountId,
) -> anyhow::Result<MarketVersion> {
    let metadata = near
        .view(market_id, "contract_source_metadata")
        .args_json(json!({}))
        .await?
        .json::<ContractSourceMetadata>()?;

    let Some(version) = metadata.version else {
        anyhow::bail!("No version string in contract source metadata for market {market_id}");
    };

    Ok(MarketVersion::from_str(&version)?)
}

#[tracing::instrument(skip(near))]
pub async fn get_market_configuration(
    near: &near_fetch::Client,
    market_id: &AccountId,
) -> anyhow::Result<MarketConfiguration> {
    Ok(near
        .view(market_id, "get_configuration")
        .args_json(json!({}))
        .await?
        .json::<MarketConfiguration>()?)
}
