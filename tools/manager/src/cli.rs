use std::path::PathBuf;

use clap::{Parser, Subcommand};
use near_account_id::AccountId;
use templar_gateway_client::Network;

use super::commands::{
    AccountNs, ContractNs, FtNs, MarketNs, OpNs, ProxyOracleGovernanceNs, ProxyOracleNs,
    ProxyOracleOwnerNs, RecoverNep141, RedstoneNs, RegistryNs, StorageNs,
};

#[derive(Parser, Debug)]
#[command(
    name = "tmplrmgr",
    version,
    about = "Gateway-native CLI frontend for Templar operations",
    arg_required_else_help = true,
    subcommand_required = true
)]
pub struct Cli {
    #[arg(
        short,
        long,
        global = true,
        env = "NETWORK",
        default_value = "testnet",
        value_name = "NETWORK"
    )]
    pub network: Network,
    #[arg(
        long,
        global = true,
        env = "RPC_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    pub rpc_url: Option<String>,
    #[arg(
        long,
        global = true,
        env = "RPC_API_KEY",
        hide_env_values = true,
        value_name = "KEY"
    )]
    pub rpc_api_key: Option<String>,
    #[arg(long, global = true, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    pub signer_id: Option<AccountId>,
    #[arg(
        long,
        global = true,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY"
    )]
    pub secret_key: Option<String>,
    #[arg(
        long,
        global = true,
        env = "GATEWAY_DATABASE_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    pub gateway_store_url: Option<String>,
    #[arg(long, global = true, env = "GATEWAY_DATABASE_MIGRATE", action = clap::ArgAction::SetTrue)]
    pub migrate_gateway_store: bool,
    #[arg(
        long,
        global = true,
        value_name = "KEY",
        requires = "gateway_store_url"
    )]
    pub idempotency_key: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum Command {
    Account {
        #[command(subcommand)]
        command: AccountNs,
    },
    Contract {
        #[command(subcommand)]
        command: ContractNs,
    },
    Registry {
        #[command(subcommand)]
        command: RegistryNs,
    },
    Storage {
        #[command(subcommand)]
        command: StorageNs,
    },
    Ft {
        #[command(subcommand)]
        command: FtNs,
    },
    Market {
        #[command(subcommand)]
        command: MarketNs,
    },
    ProxyOracle {
        #[command(subcommand)]
        command: ProxyOracleNs,
    },
    ProxyOracleOwner {
        #[command(subcommand)]
        command: ProxyOracleOwnerNs,
    },
    ProxyOracleGovernance {
        #[command(subcommand)]
        command: ProxyOracleGovernanceNs,
    },
    Redstone {
        #[command(subcommand)]
        command: RedstoneNs,
    },
    /// Recover a NEP-141 balance from the signer to a beneficiary and unregister storage.
    RecoverNep141(RecoverNep141),
    Op {
        #[command(subcommand)]
        command: OpNs,
    },
    /// Invoke a gateway read method by its full RPC name with raw JSON params.
    Read(GenericMethodCall),
    /// Invoke a gateway write method by its full RPC name with raw JSON params.
    Write(GenericMethodCall),
}

#[derive(clap::Args, Debug)]
#[command(group(clap::ArgGroup::new("params").args(["json", "json_file"]).required(true)))]
pub struct GenericMethodCall {
    /// Full gateway method name (e.g. `contract.getVersion`).
    pub method: String,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long = "json-file", value_name = "PATH")]
    pub json_file: Option<PathBuf>,
}
