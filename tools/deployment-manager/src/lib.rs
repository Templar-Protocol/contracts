pub mod batch;
pub mod commands;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use commands::{
    market::MarketArgs, proxy_oracle::ProxyOracleArgs, recover_nep141::RecoverNep141,
    registry::RegistryArgs, storage_deposit::StorageDeposit,
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

    /// Base URL for transaction explorer links (hash is appended). Defaults to
    /// the Nearblocks explorer for the selected network.
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
        let transaction_url_prefix =
            self.transaction_url_prefix
                .clone()
                .unwrap_or_else(|| match self.network {
                    Network::Mainnet => "https://nearblocks.io/txns/".to_string(),
                    Network::Testnet => "https://testnet.nearblocks.io/txns/".to_string(),
                });
        CliContext {
            workspace_path: self.workspace_dir.clone(),
            transaction_url_prefix,
            near,
        }
    }
}

pub struct CliContext {
    workspace_path: PathBuf,
    transaction_url_prefix: String,
    near: near_fetch::Client,
}

impl CliContext {
    /// Create a [`batch::BoundBatch`] that automatically logs the transaction hash and
    /// propagates execution failures when [`batch::BoundBatch::transact`] is called.
    pub fn batch<'a>(
        &self,
        signer: &'a near_crypto::Signer,
        receiver_id: &near_sdk::AccountId,
    ) -> batch::BoundBatch<'a> {
        batch::BoundBatch::new(
            self.transaction_url_prefix.clone(),
            self.near.batch(signer, receiver_id),
        )
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Manage the registry contract and its versions
    Registry(RegistryArgs),

    /// Deploy, create, and remove market contracts
    Market(MarketArgs),

    /// Deploy, create, and manage proxy oracle contracts
    ProxyOracle(ProxyOracleArgs),

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
        Commands::Registry(a) => a.run(&ctx).await?,
        Commands::Market(a) => a.run(&ctx).await?,
        Commands::ProxyOracle(a) => a.run(&ctx).await?,
        Commands::StorageDeposit(a) => a.run(&ctx).await?,
        Commands::RecoverNep141(a) => a.run(&ctx).await?,
    }

    tracing::info!("Done");
    Ok(())
}
