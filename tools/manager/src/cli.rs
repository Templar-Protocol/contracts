use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};
use templar_gateway_client::Network;
use tracing::level_filters::LevelFilter;

use templar_gateway_oracle_updates_dispatch::OracleSourceArgs;

use super::commands::signer::SignerArgs;
use super::commands::{
    AccountNs, ContractNs, FtNs, MarketNs, OracleNs, OwnerNs, ProxyOracleNs, PythNs, RecoverNep141,
    RedstoneNs, RegistryNs, SpecNs, StorageNs,
};

#[derive(Parser, Debug)]
#[command(
    name = "tmplrmgr",
    version,
    about = "Gateway-native CLI frontend for Templar operations",
    subcommand_required = true
)]
pub struct Cli {
    /// NEAR network to operate against.
    #[arg(
        short,
        long,
        global = true,
        env = "NETWORK",
        default_value_t = Network::Testnet,
        value_name = "NETWORK"
    )]
    pub network: Network,
    /// RPC endpoint override (defaults to the network's public RPC).
    #[arg(
        long,
        global = true,
        env = "RPC_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    pub rpc_url: Option<String>,
    /// API key sent with RPC requests, for providers that require one.
    #[arg(
        long,
        global = true,
        env = "RPC_API_KEY",
        hide_env_values = true,
        value_name = "KEY"
    )]
    pub rpc_api_key: Option<String>,
    /// Base URL for transaction explorer links (the tx hash is appended).
    /// Defaults to the Nearblocks explorer for the selected network.
    #[arg(long, global = true, value_name = "URL")]
    pub transaction_url_prefix: Option<String>,
    /// Reduce console log verbosity (-q = warn, -qq = error, -qqq = off).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub quiet: u8,
    /// Increase console log verbosity (-v = debug, -vv = trace).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Console log level derived from the `-q`/`-v` counts (default INFO).
    pub fn console_level(&self) -> LevelFilter {
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

    /// Explorer base URL for tx links, defaulting per network when unset.
    pub fn transaction_url_prefix(&self) -> String {
        self.transaction_url_prefix.clone().unwrap_or_else(|| {
            match self.network {
                Network::Mainnet => "https://nearblocks.io/txns/",
                Network::Testnet => "https://testnet.nearblocks.io/txns/",
            }
            .to_owned()
        })
    }
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum Command {
    /// Account reads and deletion.
    Account {
        #[command(subcommand)]
        command: AccountNs,
    },
    /// Inspect deployed contract versions.
    Contract {
        #[command(subcommand)]
        command: ContractNs,
    },
    /// Manage a contract registry: versions and deployments.
    Registry {
        #[command(subcommand)]
        command: RegistryNs,
    },
    /// NEP-145 storage management on a contract.
    Storage {
        #[command(subcommand)]
        command: StorageNs,
    },
    /// NEP-141 fungible-token reads and transfers.
    Ft {
        #[command(subcommand)]
        command: FtNs,
    },
    /// Deploy and tear down markets.
    Market {
        #[command(subcommand)]
        command: MarketNs,
    },
    /// Deploy, inspect, update, and govern proxy oracles.
    ProxyOracle {
        #[command(subcommand)]
        command: ProxyOracleNs,
    },
    /// Manage a contract implementing the Owner interface.
    Owner {
        #[command(subcommand)]
        command: OwnerNs,
    },
    /// Push oracle price updates, fetching their payloads inside the gateway.
    Oracle {
        #[command(subcommand)]
        command: OracleNs,
    },
    /// Read and update a Pyth oracle contract directly.
    Pyth {
        #[command(subcommand)]
        command: PythNs,
    },
    /// Deploy and operate a RedStone price adapter.
    Redstone {
        #[command(subcommand)]
        command: RedstoneNs,
    },
    /// Recover a NEP-141 balance from the signer to a beneficiary and unregister storage.
    RecoverNep141(RecoverNep141),
    /// Work with a declarative market deployment spec. Entirely local.
    Spec {
        #[command(subcommand)]
        command: SpecNs,
    },
    /// Invoke a gateway read method by its full RPC name with raw JSON params.
    Read(GenericMethodCall),
    /// Invoke a gateway write method by its full RPC name with raw JSON params.
    Write(WriteMethodCall),
}

#[derive(clap::Args, Debug)]
#[command(group(clap::ArgGroup::new("params").args(["json", "json_file"]).required(true)))]
pub struct GenericMethodCall {
    /// Full gateway method name (e.g. `contract.getVersion`).
    pub method: String,
    /// Method params as an inline JSON object.
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    /// Method params read from a JSON file.
    #[arg(long, value_name = "PATH")]
    pub json_file: Option<PathBuf>,
}

/// A generic write invocation: the method call plus the signer credentials it
/// requires. Reads flatten only [`GenericMethodCall`], so credentials are an
/// unexpected argument there.
///
/// The oracle source flags are here because `write oracle.<method>` may reach an
/// in-process payload source. Only the method actually dispatched builds a source, so
/// `write market.borrow` needs no Lazer token.
#[derive(clap::Args, Debug)]
pub struct WriteMethodCall {
    #[command(flatten)]
    pub call: GenericMethodCall,
    #[command(flatten)]
    pub oracle_sources: OracleSourceArgs,
    #[command(flatten)]
    pub signer: SignerArgs,
}
