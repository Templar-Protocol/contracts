use std::borrow::Cow;

use anyhow::Context;
use clap::{ArgGroup, Args};
use near_sdk::{AccountId, NearToken};
use templar_common::registry::DeployMode;
use templar_contract_artifacts::{artifact_value_parser, find_by_id, ArtifactId};
use templar_gateway_types::RegistryVersion;
use templar_tools_common::near::{self, Function};

use crate::{
    util::{ContractLoader, SignerArgs},
    CliContext,
};

const STORAGE_AMOUNT_PER_BYTE: NearToken = NearToken::from_yoctonear(10_000_000_000_000_000_000);

#[allow(clippy::struct_excessive_bools)]
#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("artifact_selector")
        .args(["market", "uac", "proxy_oracle", "redstone_adapter", "package"])
        .required(true)
        .multiple(false)
))]
pub struct Package {
    /// Market contract
    #[arg(long)]
    pub market: bool,
    /// Universal account contract
    #[arg(long)]
    pub uac: bool,
    /// Proxy oracle contract
    #[arg(long)]
    pub proxy_oracle: bool,
    /// RedStone adapter contract
    #[arg(long)]
    pub redstone_adapter: bool,
    /// Specify a contract by package name or artifact ID
    #[arg(long, visible_alias = "artifact")]
    pub package: Option<String>,
}

impl Package {
    pub fn artifact(&self) -> Option<ArtifactId> {
        if self.market {
            Some(ArtifactId::Market)
        } else if self.uac {
            Some(ArtifactId::UniversalAccount)
        } else if self.proxy_oracle {
            Some(ArtifactId::ProxyOracle)
        } else if self.redstone_adapter {
            Some(ArtifactId::RedstoneAdapter)
        } else {
            self.package
                .as_deref()
                .and_then(|package| artifact_value_parser(package).ok())
        }
    }

    pub fn package_name(&self) -> Cow<'_, str> {
        if let Some(artifact) = self.artifact() {
            find_by_id(artifact).map_or_else(
                |_| Cow::Owned(format!("<unknown-artifact:{artifact:?}>")),
                |metadata| Cow::Borrowed(metadata.package_name),
            )
        } else {
            Cow::Borrowed(self.package.as_deref().unwrap_or_default())
        }
    }

    pub fn load<V>(
        &self,
        loader: &ContractLoader,
    ) -> anyhow::Result<templar_tools_common::build::LoadedContract<V>> {
        if let Some(artifact) = self.artifact() {
            loader.load_artifact(artifact)
        } else {
            loader.load(self.package.as_deref().unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct PackageCommand {
        #[command(flatten)]
        package: Package,
    }

    fn empty_package() -> Package {
        Package {
            market: false,
            uac: false,
            proxy_oracle: false,
            redstone_adapter: false,
            package: None,
        }
    }

    #[test]
    fn package_selector_resolves_package_name_when_artifact_name_is_provided() {
        let package = Package {
            package: Some("market".to_string()),
            ..empty_package()
        };

        assert_eq!(package.artifact(), Some(ArtifactId::Market));
        assert_eq!(package.package_name(), "templar-market-contract");
    }

    #[test]
    fn legacy_market_flag_resolves_package_name_when_used() {
        let package = Package {
            market: true,
            ..empty_package()
        };

        assert_eq!(package.artifact(), Some(ArtifactId::Market));
        assert_eq!(package.package_name(), "templar-market-contract");
    }

    #[test]
    fn package_selector_preserves_custom_package_name_when_used() {
        let package = Package {
            package: Some("custom-contract".to_string()),
            ..empty_package()
        };

        assert_eq!(package.artifact(), None);
        assert_eq!(package.package_name(), "custom-contract");
    }

    #[test]
    fn package_selector_rejects_conflicting_inputs() {
        let error =
            PackageCommand::try_parse_from(["tmplrmgr", "--market", "--package", "proxy-oracle"]);

        assert!(error.is_err());
    }
}

#[derive(Args)]
pub struct AddVersion {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub contract_wasm: ContractLoader,
    #[command(flatten)]
    pub package: Package,
    /// Registry contract account ID
    #[arg(long)]
    pub registry_id: AccountId,
    /// Version key to store in the registry
    ///
    /// If not provided, the version key will be derived from the package metadata.
    #[arg(long)]
    pub version_key: Option<String>,
    /// Deployment mode
    #[arg(long)]
    pub deploy_mode: DeployMode,
    /// Deposit to attach in NEAR. If not provided, it will be estimated based
    /// on the contract size and the deploy mode.
    #[arg(long)]
    pub deposit: Option<NearToken>,
}

impl AddVersion {
    #[tracing::instrument(skip_all, name = "add_version", fields(account_id = %self.signer.account_id, package = %self.package.package_name(), registry_id = %self.registry_id, deploy_mode = %self.deploy_mode))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let loaded_contract = self.package.load::<()>(&self.contract_wasm)?;
        tracing::debug!(loaded_contract_version = %loaded_contract.version, "Loaded contract");
        let registry_version: RegistryVersion =
            near::contract_version(&ctx.near, &self.registry_id).await?;
        tracing::debug!(%registry_version, "Loaded registry");
        if !registry_version.supports_global_contracts() && self.deploy_mode != DeployMode::Normal {
            anyhow::bail!(
                "Registry version {} does not support global contracts, but deploy mode {:?} was requested",
                registry_version,
                self.deploy_mode
            );
        }
        let version_key = self
            .version_key
            .clone()
            .unwrap_or_else(|| loaded_contract.version_key());
        tracing::debug!(%version_key);
        let borsh_args = registry_version.encode_add_version_args(
            &version_key,
            self.deploy_mode,
            &loaded_contract.wasm_bytes,
        )?;
        let estimated_deposit = if self.deploy_mode == DeployMode::GlobalHash {
            STORAGE_AMOUNT_PER_BYTE.saturating_mul(loaded_contract.wasm_bytes.len() as u128 * 10)
        } else {
            NearToken::from_yoctonear(1)
        };
        let deposit = self.deposit.unwrap_or(estimated_deposit);
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
