use std::path::PathBuf;

use anyhow::{bail, Context as _};
use clap::{Args, ValueEnum};
use near_account_id::AccountId;
use templar_common::oracle::redstone::{config, Config};
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::version::RedstoneAdapterVersion;

use crate::commands::deploy_common::DeployCommonArgs;
use crate::commands::signer::SignerArgs;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Preset {
    Prod,
    Test,
}

/// Deploy a RedStone price adapter from a registered version, granting the
/// signer a full access key so the operator retains control of the new account.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("redstone_config")
        .args(["preset", "init_args", "init_args_file"])
        .required(true)
        .multiple(false)
))]
pub struct Create {
    #[command(flatten)]
    common: DeployCommonArgs,
    /// Built-in RedStone configuration to use.
    #[arg(long, value_enum)]
    preset: Option<Preset>,
    /// Account to seed with the adapter's administration roles.
    #[arg(
        long,
        value_name = "ACCOUNT_ID",
        conflicts_with_all = ["init_args", "init_args_file"]
    )]
    admin_id: Option<AccountId>,
    /// Full init args JSON, passed to the contract verbatim.
    ///
    /// The RedStone adapter `0.2.0` initializer requires an `admin_id`.
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to full init args JSON, passed to the contract verbatim.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

/// Reject a target adapter version whose `new` does not require `admin_id`.
///
/// Only as good as the version key, which is a convention the registry does not
/// enforce (see [`Version::from_version_key`]). An operator guardrail, hence the
/// CLI and not the gateway; ENG-463 replaces it with a check against the
/// contract's own ABI.
///
/// [`Version::from_version_key`]: templar_gateway_types::version::Version::from_version_key
fn check_new_requires_admin_id(version_key: &str) -> anyhow::Result<()> {
    let version = RedstoneAdapterVersion::from_version_key(version_key)
        .with_context(|| format!("cannot tell whether {version_key} requires admin_id"))?;

    if !version.new_requires_admin_id() {
        bail!(
            "RedStone adapter {version} does not require admin_id, so a missing custom init \
             argument could seed the registry. Deploy >= 0.2.0."
        );
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct RedstoneInitArgs {
    config: Config,
    admin_id: AccountId,
}

impl Create {
    fn preset_init_args(&self, config: Config) -> anyhow::Result<RedstoneInitArgs> {
        let admin_id = self
            .admin_id
            .clone()
            .context("--preset requires --admin-id")?;

        Ok(RedstoneInitArgs { config, admin_id })
    }

    pub fn try_into_spec(self) -> anyhow::Result<registry_spec::Deploy> {
        let init_args = if let Some(preset) = self.preset {
            let config = match preset {
                Preset::Prod => config::prod(),
                Preset::Test => config::test(),
            };
            let init_args = self.preset_init_args(config)?;
            serde_json::to_vec(&init_args).context("serialize RedStone preset init args")?
        } else if let Some(args) = self.init_args {
            args.into_bytes()
        } else if let Some(path) = self.init_args_file {
            std::fs::read(&path)
                .with_context(|| format!("read RedStone init args from {}", path.display()))?
        } else {
            bail!("provide --preset, --init-args, or --init-args-file");
        };
        let signer = self.signer;
        let deploy = self.common.into_deploy(|| signer.public_key(), init_args)?;

        check_new_requires_admin_id(&deploy.version_key)?;

        Ok(deploy)
    }
}
