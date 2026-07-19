use std::path::PathBuf;

use anyhow::{bail, Context as _};
use clap::{Args, ValueEnum};
use near_account_id::AccountId;
use templar_common::oracle::redstone::{config, Config};
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::version::RedstoneAdapterVersion;

use crate::commands::deploy_common::DeployCommonArgs;
use crate::commands::signer::SignerArgs;

/// Deploy a RedStone price adapter from a registered version, granting the
/// signer a full access key so the operator retains control of the new account.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Preset {
    Prod,
    Test,
}

#[derive(Args, Debug)]
#[command(
    group(
        clap::ArgGroup::new("redstone_config")
            .args(["preset", "init_args", "init_args_file"])
            .required(true)
            .multiple(false)
    ),
    group(
        clap::ArgGroup::new("redstone_admin")
            .args(["admin_id", "predecessor_is_admin"])
            .multiple(false)
    )
)]
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
        conflicts_with_all = ["init_args", "init_args_file", "predecessor_is_admin"]
    )]
    admin_id: Option<AccountId>,
    /// Seed the registry predecessor with administration roles.
    #[arg(long, conflicts_with_all = ["init_args", "init_args_file"])]
    predecessor_is_admin: bool,
    /// Full init args JSON, passed to the contract verbatim.
    ///
    /// Use `admin_id: null` to explicitly seed the predecessor.
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to full init args JSON, passed to the contract verbatim.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

/// Temporarily use the version key as an ABI capability guard. Remove this
/// check once ENG-463 validates registry-deploy init arguments against the
/// contract's embedded ABI.
fn check_admin_id_is_honored(version_key: &str) -> anyhow::Result<()> {
    let version = RedstoneAdapterVersion::from_version_key(version_key)
        .with_context(|| format!("cannot tell whether {version_key} honors admin_id"))?;

    if !version.new_accepts_admin_id() {
        bail!(
            "RedStone adapter {version} cannot honor admin_id: its new only accepts config, so \
             the registry would receive the admin roles. Deploy >= 0.1.1."
        );
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct InitArgs {
    config: Config,
    admin_id: Option<AccountId>,
}

impl Create {
    fn preset_init_args(&self, config: Config) -> anyhow::Result<InitArgs> {
        let admin_id = match (&self.admin_id, self.predecessor_is_admin) {
            (Some(admin_id), _) => Some(admin_id.clone()),
            (None, true) => None,
            (None, false) => {
                bail!("--preset requires either --admin-id or --predecessor-is-admin")
            }
        };

        Ok(InitArgs { config, admin_id })
    }

    pub fn try_into_spec(self) -> anyhow::Result<registry_spec::Deploy> {
        let signer_public_key = self.signer.public_key()?;
        let (init_args, seats_admin) = if let Some(preset) = self.preset {
            let config = match preset {
                Preset::Prod => config::prod(),
                Preset::Test => config::test(),
            };
            let init_args = self.preset_init_args(config)?;
            (
                serde_json::to_vec(&init_args).context("serialize RedStone preset init args")?,
                init_args.admin_id.is_some(),
            )
        } else if let Some(args) = self.init_args {
            (args.into_bytes(), false)
        } else if let Some(path) = self.init_args_file {
            (
                std::fs::read(&path)
                    .with_context(|| format!("read RedStone init args from {}", path.display()))?,
                false,
            )
        } else {
            bail!("provide --preset, --init-args, or --init-args-file");
        };
        let deploy = self.common.into_deploy(signer_public_key, init_args);

        if seats_admin {
            check_admin_id_is_honored(&deploy.version_key)?;
        }

        Ok(deploy)
    }
}
