pub mod batch;
pub mod commands;

use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};
use commands::{
    market::MarketArgs, proxy_oracle::ProxyOracleArgs, recover_nep141::RecoverNep141,
    redstone_adapter::RedStoneAdapterArgs, registry::RegistryArgs, storage_deposit::StorageDeposit,
};
use templar_common::utils::Network;

pub use templar_tools_common::near;
use tracing::level_filters::LevelFilter;

#[derive(Parser)]
#[command(version, about = "CLI tool for deploying and managing Templar markets")]
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

    /// Increase log verbosity (-v = info, -vv = debug, -vvv = trace)
    #[arg(short, long, action = ArgAction::Count, global = true)]
    verbose: u8,

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
    /// Create a new `CliContext` with the given RPC URL and workspace path.
    pub fn new(rpc_url: &str, workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            transaction_url_prefix: String::new(),
            near: near_fetch::Client::new(rpc_url),
        }
    }

    /// Access the underlying NEAR RPC client.
    pub fn near(&self) -> &near_fetch::Client {
        &self.near
    }

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

fn init_tracing(verbose: u8) {
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    let console_default = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    let console_filter = EnvFilter::builder()
        .with_default_directive(console_default.into())
        .from_env_lossy();

    let console_layer = fmt::layer().with_filter(console_filter);

    let registry = tracing_subscriber::registry().with(console_layer);

    // Attempt to set up file logging; fall back to console-only if it fails.
    let log_dir = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join(env!("CARGO_PKG_NAME")).join("logs"));

    if let Some(log_dir) = log_dir {
        let file_appender = tracing_appender::rolling::daily(&log_dir, "log");
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_writer(file_appender)
            .with_filter(LevelFilter::DEBUG);
        registry.with(file_layer).init();
        tracing::debug!(path = %log_dir.display(), "File logging enabled");
    } else {
        registry.init();
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

    /// Deploy, create, and manage RedStone adapter contracts
    RedstoneAdapter(RedStoneAdapterArgs),

    /// Perform a storage deposit on a contract on behalf of an account
    StorageDeposit(StorageDeposit),

    /// Recover NEP-141 tokens from an account and unregister its storage slot
    RecoverNep141(RecoverNep141),
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    tracing::info!(network = %cli.network, "Connecting");

    let ctx = cli.make_context();

    match cli.command {
        Commands::Registry(a) => a.run(&ctx).await?,
        Commands::Market(a) => a.run(&ctx).await?,
        Commands::ProxyOracle(a) => a.run(&ctx).await?,
        Commands::RedstoneAdapter(a) => a.run(&ctx).await?,
        Commands::StorageDeposit(a) => a.run(&ctx).await?,
        Commands::RecoverNep141(a) => a.run(&ctx).await?,
    }

    Ok(())
}
