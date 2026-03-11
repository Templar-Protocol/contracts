pub mod commands;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use commands::{
    add_version::{AddMarketVersion, AddUacVersion, AddVersion},
    deploy_from_registry::DeployFromRegistry,
    deploy_registry::DeployRegistry,
    recover_nep141::RecoverNep141,
    remove_all_markets::RemoveAllMarkets,
    remove_all_versions::RemoveAllVersions,
    remove_market::RemoveMarket,
    remove_registry::RemoveRegistry,
    remove_version::RemoveVersion,
    storage_deposit::StorageDeposit,
};
use templar_common::utils::Network;
/// Re-export shared NEAR client utilities so command modules can use `crate::near`.
pub use templar_tools_common::near;

#[derive(Parser)]
#[command(
    name = "market-deployer",
    version,
    about = "CLI tool for deploying and managing Templar markets"
)]
struct Cli {
    /// NEAR network to connect to
    #[arg(short, long, env = "NETWORK", default_value_t = Network::Testnet)]
    network: Network,

    /// Override the default RPC URL for the selected network
    #[arg(long, env = "RPC_URL")]
    rpc_url: Option<String>,

    /// Specify a URL for transaction links. The transaction hash will be appended to this URL.
    #[arg(long)]
    transaction_url_prefix: Option<String>,

    /// Path to the workspace root (defaults to current directory)
    #[arg(short, long, env = "WORKSPACE_DIR", default_value = ".")]
    workspace_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    fn make_context(&self) -> CliContext {
        let near = near_fetch::Client::new(
            self.rpc_url
                .as_deref()
                .unwrap_or_else(|| self.network.rpc_url()),
        );
        CliContext {
            workspace_path: self.workspace_dir.clone(),
            transaction_url_prefix: match self.network {
                Network::Mainnet => "https://nearblocks.io/txns/".to_string(),
                Network::Testnet => "https://testnet.nearblocks.io/txns/".to_string(),
            },
            near,
        }
    }
}

pub struct CliContext {
    workspace_path: PathBuf,
    transaction_url_prefix: String,
    near: near_fetch::Client,
}

#[derive(Subcommand)]
enum Commands {
    /// Build and deploy the registry contract with an initialization call
    DeployRegistry(DeployRegistry),

    /// Add a contract version to the registry (wasm must already be built)
    AddVersion(AddVersion),

    /// Build the market contract and register it as a new version
    AddMarketVersion(AddMarketVersion),

    /// Build the universal-account contract and register it as a new version
    AddUacVersion(AddUacVersion),

    /// Deploy a market from the registry
    DeployFromRegistry(DeployFromRegistry),

    /// Remove a market: recover NEP-141 tokens then delete the account
    RemoveMarket(RemoveMarket),

    /// Remove all versions from a registry then delete the registry account
    RemoveRegistry(RemoveRegistry),

    /// Remove every market listed in a registry
    RemoveAllMarkets(RemoveAllMarkets),

    /// Remove all versions stored in a registry
    RemoveAllVersions(RemoveAllVersions),

    /// Remove a single version from a registry
    RemoveVersion(RemoveVersion),

    /// Perform a storage deposit on a contract on behalf of an account
    StorageDeposit(StorageDeposit),

    /// Recover NEP-141 tokens from an account and unregister its storage slot
    RecoverNep141(RecoverNep141),
}

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    tracing::info!(network = %cli.network, "Connecting");

    let ctx = cli.make_context();

    match cli.command {
        Commands::DeployRegistry(a) => a.run(&ctx).await?,
        Commands::AddVersion(a) => a.run(&ctx).await?,
        Commands::AddMarketVersion(a) => a.run(&ctx).await?,
        Commands::AddUacVersion(a) => a.run(&ctx).await?,
        Commands::DeployFromRegistry(a) => a.run(&ctx).await?,
        Commands::RemoveMarket(a) => a.run(&ctx).await?,
        Commands::RemoveRegistry(a) => a.run(&ctx).await?,
        Commands::RemoveAllMarkets(a) => a.run(&ctx).await?,
        Commands::RemoveAllVersions(a) => a.run(&ctx).await?,
        Commands::RemoveVersion(a) => a.run(&ctx).await?,
        Commands::StorageDeposit(a) => a.run(&ctx).await?,
        Commands::RecoverNep141(a) => a.run(&ctx).await?,
    }

    tracing::info!("Done");
    Ok(())
}
