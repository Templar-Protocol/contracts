pub mod batch;
pub mod commands;
pub mod util;

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};
use commands::{
    market::MarketArgs, proxy_oracle::ProxyOracleArgs, recover_nep141::RecoverNep141,
    redstone_adapter::RedStoneAdapterArgs, registry::RegistryArgs, storage_deposit::StorageDeposit,
};
use templar_common::utils::Network;
use tracing::level_filters::LevelFilter;

pub use templar_tools_common::near;

#[allow(async_fn_in_trait)]
pub trait Runner<Input> {
    type Output;

    async fn run(&self, ctx: &crate::CliContext, input: &Input) -> anyhow::Result<Self::Output>;
}

#[derive(Parser)]
#[command(group(ArgGroup::new("verbosity").multiple(false).args(["quiet", "verbose"])))]
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

    #[command(flatten)]
    verbosity: VerbosityArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug, Default)]
struct VerbosityArgs {
    /// Reduce console log verbosity (-q = warn, -qq = error, -qqq = off)
    #[arg(short, long, action = ArgAction::Count, conflicts_with = "verbose")]
    quiet: u8,

    /// Increase console log verbosity (-v = debug, -vv = trace)
    #[arg(short, long, action = ArgAction::Count, conflicts_with = "quiet")]
    verbose: u8,
}

impl VerbosityArgs {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !(self.quiet > 0 && self.verbose > 0),
            "Only one of --quiet or --verbose may be specified"
        );
        Ok(())
    }

    fn console_level(&self) -> LevelFilter {
        const DEFAULT_LEVEL: u8 = 3;
        match DEFAULT_LEVEL
            .saturating_sub(self.quiet)
            .saturating_add(self.verbose)
        {
            0 => LevelFilter::OFF,
            1 => LevelFilter::ERROR,
            2 => LevelFilter::WARN,
            3 => LevelFilter::INFO,
            4 => LevelFilter::DEBUG,
            5.. => LevelFilter::TRACE,
        }
    }
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
            transaction_url_prefix,
            near,
        }
    }
}

pub struct CliContext {
    pub transaction_url_prefix: String,
    pub near: near_fetch::Client,
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

fn init_tracing(console_default: LevelFilter) {
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    let console_filter = EnvFilter::builder()
        .with_default_directive(console_default.into())
        .from_env_lossy();

    let console_layer = fmt::layer().with_filter(console_filter);

    let registry = tracing_subscriber::registry().with(console_layer);

    // Attempt to set up file logging; fall back to console-only if it fails.
    let file_layer = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join(env!("CARGO_PKG_NAME")).join("logs"))
        .and_then(|log_dir| {
            tracing::debug!(log_dir = %log_dir.display(), "Initializing file logging");
            tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("log")
                .build(&log_dir)
                .inspect_err(|e| {
                    tracing::warn!(error = %e, "Failed to initialize file logging");
                })
                .ok()
        })
        .map(|file_appender| {
            fmt::layer()
                .with_ansi(false)
                .with_writer(file_appender)
                .with_filter(LevelFilter::DEBUG)
        });

    if let Some(file_layer) = file_layer {
        registry.with(file_layer).init();
    } else {
        tracing::warn!("Failed to initialize file logging");
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
    cli.verbosity.validate()?;

    init_tracing(cli.verbosity.console_level());

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
