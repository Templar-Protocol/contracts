use std::path::PathBuf;

use anyhow::{bail, Context as _};
use clap::Args;
use serde_json::{json, Value};
use templar_common::oracle::redstone::config;
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::{version::RedstoneAdapterVersion, ManagedAccountId};

use crate::commands::deploy_common::DeployCommonArgs;
use crate::commands::signer::SignerArgs;

/// Deploy a RedStone price adapter from a registered version, granting the
/// signer a full access key so the operator retains control of the new account.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("redstone_config")
        .args(["prod", "test", "init_args", "init_args_file"])
        .required(true)
        .multiple(false)
))]
pub struct Create {
    #[command(flatten)]
    common: DeployCommonArgs,
    /// Use the built-in production RedStone configuration.
    #[arg(long)]
    prod: bool,
    /// Use the built-in test RedStone configuration.
    #[arg(long)]
    test: bool,
    /// Full init args JSON. When `admin_id` is omitted, it defaults to the signer.
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to a full init args JSON file.
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

fn init_args_with_default_admin(
    mut init_args: Value,
    admin_id: &ManagedAccountId,
) -> anyhow::Result<(Vec<u8>, bool)> {
    let args = init_args
        .as_object_mut()
        .context("RedStone init args must be a JSON object")?;
    let seats_admin = if args.contains_key("admin_id") {
        !args["admin_id"].is_null()
    } else {
        args.insert(
            "admin_id".to_owned(),
            serde_json::to_value(admin_id).context("serialize RedStone admin_id")?,
        );
        true
    };

    Ok((
        serde_json::to_vec(&init_args).context("serialize RedStone init args")?,
        seats_admin,
    ))
}

impl Create {
    pub fn try_into_spec(self) -> anyhow::Result<registry_spec::Deploy> {
        let signer_public_key = self.signer.public_key()?;
        let init_args = if self.prod {
            json!({ "config": config::prod() })
        } else if self.test {
            json!({ "config": config::test() })
        } else if let Some(args) = self.init_args {
            serde_json::from_str(&args).context("parse RedStone init args")?
        } else if let Some(path) = self.init_args_file {
            let args = std::fs::read(&path)
                .with_context(|| format!("read RedStone init args from {}", path.display()))?;
            serde_json::from_slice(&args).context("parse RedStone init args")?
        } else {
            bail!("provide --prod, --test, --init-args, or --init-args-file");
        };
        let (init_args, seats_admin) =
            init_args_with_default_admin(init_args, &self.signer.account_id())?;
        let deploy = self.common.into_deploy(signer_public_key, init_args);

        if seats_admin {
            check_admin_id_is_honored(&deploy.version_key)?;
        }

        Ok(deploy)
    }
}
