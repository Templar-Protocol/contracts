pub mod commands;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use commands::{
    add_version::AddVersion, deploy_from_registry::DeployFromRegistry,
    deploy_registry::DeployRegistry, SignerArgs,
};
use near_crypto::SecretKey;
use near_sdk::{AccountId, NearToken};
use templar_common::{registry::DeployMode, utils::Network};
/// Re-export shared NEAR client utilities so command modules can use `crate::near`.
pub use tools_common::near;

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

    /// Path to the workspace root (defaults to current directory)
    #[arg(long, env = "WORKSPACE_DIR", default_value = ".")]
    workspace_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    pub fn make_context(&self) -> anyhow::Result<Context> {
        let near = near_fetch::Client::new(
            self.rpc_url
                .as_deref()
                .unwrap_or_else(|| self.network.rpc_url()),
        );
        let workspace_path = self.workspace_dir.clone();
        Ok(Context {
            workspace_path,
            near,
        })
    }
}

struct CliContext {
    workspace_path: PathBuf,
    near: near_fetch::Client,
}

impl CliContext {
    pub fn contract_wasm_path(&self, package: &str) -> PathBuf {
        self.workspace_path
            .join("target/near")
            .join(package)
            .join(format!("{}.wasm", package))
    }

    pub fn contract_wasm(&self, package: &str) -> std::io::Result<Vec<u8>> {
        let path = self.contract_wasm_path(package);
        tracing::info!(path = %path.display(), "Reading wasm");
        std::fs::read(path)
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Build and deploy the registry contract with an initialisation call
    DeployRegistry(DeployRegistry),

    /// Add a contract version to the registry (wasm must already be built)
    AddVersion(AddVersion),

    /// Build the market contract and register it as a new version
    AddMarketVersion(AddMarketVersionArgs),

    /// Build the universal-account contract and register it as a new version
    AddUacVersion(AddUacVersionArgs),

    /// Deploy a market from the registry
    DeployFromRegistry(DeployFromRegistry),

    /// Remove a market: recover NEP-141 tokens then delete the account
    RemoveMarket(RemoveMarketArgs),

    /// Remove all versions from a registry then delete the registry account
    RemoveRegistry(RemoveRegistryArgs),

    /// Remove every market listed in a registry
    RemoveAllMarkets(RemoveAllMarketsArgs),

    /// Remove all versions stored in a registry
    RemoveAllVersions(RemoveAllVersionsArgs),

    /// Remove a single version from a registry
    RemoveVersion(RemoveVersionArgs),

    /// Perform a storage deposit on a contract on behalf of an account
    StorageDeposit(StorageDepositArgs),

    /// Recover NEP-141 tokens from an account and unregister its storage slot
    RecoverNep141(RecoverNep141Args),
}

// ── per-command argument structs ─────────────────────────────────────────────

#[derive(Args)]
struct AddMarketVersionArgs {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    #[arg(long)]
    version_key: String,
    #[arg(long, default_value = "normal")]
    deploy_mode: DeployMode,
    #[arg(long)]
    deposit: Option<NearToken>,
}

#[derive(Args)]
struct AddUacVersionArgs {
    #[command(flatten)]
    signer: SignerArgs,
    /// Registry contract account ID (defaults to --account-id)
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: Option<AccountId>,
    #[arg(long)]
    version_key: String,
    #[arg(long, default_value = "global-hash")]
    deploy_mode: DeployMode,
    #[arg(long)]
    deposit: Option<NearToken>,
}

#[derive(Args)]
struct RemoveRegistryArgs {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    beneficiary_id: AccountId,
}
#[derive(Args)]
struct RemoveAllVersionsArgs {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
}

#[derive(Args)]
struct RemoveVersionArgs {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    #[arg(long)]
    version_key: String,
}

#[derive(Args)]
struct StorageDepositArgs {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    contract_id: AccountId,
}
