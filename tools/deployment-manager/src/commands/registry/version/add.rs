use anyhow::Context;
use clap::Args;
use near_fetch::ops::Function;
use near_sdk::{AccountId, NearToken};
use templar_common::registry::DeployMode;
use templar_tools_common::{near, version::RegistryVersion};

use crate::commands::{FixedContractWasm, SignerArgs};
use crate::CliContext;

const MARKET_PACKAGE: &str = "templar-market-contract";
const UAC_PACKAGE: &str = "templar-universal-account-contract";
const PROXY_ORACLE_PACKAGE: &str = "templar-proxy-oracle-contract";
const REDSTONE_ADAPTER_PACKAGE: &str = "templar-redstone-adapter-contract";

const STORAGE_AMOUNT_PER_BYTE: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000);

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Package {
    /// Market contract
    #[arg(long)]
    market: bool,
    /// Universal account contract
    #[arg(long)]
    uac: bool,
    /// Proxy oracle contract
    #[arg(long)]
    proxy_oracle: bool,
    /// RedStone adapter contract
    #[arg(long)]
    redstone_adapter: bool,
    /// Specify a contract by package name
    #[arg(long)]
    package: Option<String>,
}

impl Package {
    pub fn package(&self) -> &str {
        if self.market {
            MARKET_PACKAGE
        } else if self.uac {
            UAC_PACKAGE
        } else if self.proxy_oracle {
            PROXY_ORACLE_PACKAGE
        } else if self.redstone_adapter {
            REDSTONE_ADAPTER_PACKAGE
        } else {
            self.package.as_deref().unwrap_or_default()
        }
    }
}

#[derive(Args, Debug)]
pub struct AddVersion {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    contract_wasm: FixedContractWasm,
    #[command(flatten)]
    package: Package,
    /// Registry contract account ID
    #[arg(long)]
    registry_id: AccountId,
    /// Version key to store in the registry
    ///
    /// If not provided, the version key will be derived from the package version.
    #[arg(long)]
    version_key: Option<String>,
    /// Deployment mode
    #[arg(long, default_value_t = DeployMode::Normal)]
    deploy_mode: DeployMode,
    /// Deposit to attach in NEAR. If not provided, it will be estimated based
    /// on the contract size and the deploy mode.
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl AddVersion {
    #[tracing::instrument(skip_all, name = "add_version", fields(account_id = %self.signer.account_id, package = %self.package.package(), registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let loaded_contract = self
            .contract_wasm
            .load_contract::<()>(ctx, self.package.package())?;
        tracing::debug!(loaded_contract_version = %loaded_contract.version, "Loaded contract");
        let registry_version: RegistryVersion =
            near::contract_version(&ctx.near, &self.registry_id).await?;
        tracing::debug!(%registry_version, "Loaded registry");
        let deploy_mode = if registry_version.supports_global_contracts() {
            self.deploy_mode
        } else {
            DeployMode::Normal
        };
        tracing::debug!(%deploy_mode);
        let version_key = self
            .version_key
            .clone()
            .unwrap_or_else(|| loaded_contract.version_key());
        tracing::debug!(%version_key);
        let borsh_args = registry_version.encode_add_version_args(
            &version_key,
            deploy_mode,
            &loaded_contract.wasm_bytes,
        )?;
        let deposit = if deploy_mode == DeployMode::GlobalHash {
            self.deposit.unwrap_or(
                STORAGE_AMOUNT_PER_BYTE
                    .saturating_mul(loaded_contract.wasm_bytes.len() as u128 * 10),
            )
        } else {
            NearToken::from_yoctonear(1)
        };
        tracing::debug!(%deposit);
        tracing::info!(%version_key, "Calling add_version on registry");
        let signer = self.signer.signer();
        ctx.batch(&signer, &self.registry_id)
            .call(
                Function::new("add_version")
                    .args(borsh_args)
                    .deposit(deposit)
                    .max_gas(),
            )
            .transact()
            .await
            .context("add_version")?;
        tracing::info!(%version_key, "Version registered");

        Ok(())
    }
}
