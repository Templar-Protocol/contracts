use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use serde_json::json;
use templar_common::oracle::redstone::config;
use templar_gateway_methods_spec::registry as registry_spec;

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
    /// Full init args JSON (`{"config": ..., "admin_id": ...}`).
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to a full init args JSON file.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Create {
    pub fn try_into_spec(self) -> anyhow::Result<registry_spec::Deploy> {
        let signer_public_key = self.signer.public_key()?;
        let admin_id = self.signer.account_id();
        let init_bytes = if self.prod {
            serde_json::to_vec(&json!({
                "config": config::prod(),
                "admin_id": admin_id,
            }))
            .context("serialize prod config")?
        } else if self.test {
            serde_json::to_vec(&json!({
                "config": config::test(),
                "admin_id": admin_id,
            }))
            .context("serialize test config")?
        } else if let Some(args) = self.init_args {
            args.into_bytes()
        } else if let Some(path) = self.init_args_file {
            std::fs::read(&path)
                .with_context(|| format!("read RedStone init args from {}", path.display()))?
        } else {
            anyhow::bail!("provide --prod, --test, --init-args, or --init-args-file");
        };

        Ok(self.common.into_deploy(signer_public_key, init_bytes))
    }
}
