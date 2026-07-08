use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use serde_json::json;
use templar_common::oracle::redstone::config;
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::{Base64Bytes, NearToken};

#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("redstone_config")
        .args(["prod", "test", "init_args", "init_args_file"])
        .required(true)
        .multiple(false)
))]
pub struct Create {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// Use the built-in production RedStone configuration.
    #[arg(long)]
    prod: bool,
    /// Use the built-in test RedStone configuration.
    #[arg(long)]
    test: bool,
    /// Full init args JSON (`{"config": ...}`).
    #[arg(long, value_name = "JSON")]
    init_args: Option<String>,
    /// Path to a full init args JSON file.
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<PathBuf>,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Create {
    pub fn parse(self) -> anyhow::Result<registry_spec::Deploy> {
        let init_bytes = if self.prod {
            serde_json::to_vec(&json!({ "config": config::prod() }))
                .context("serialize prod config")?
        } else if self.test {
            serde_json::to_vec(&json!({ "config": config::test() }))
                .context("serialize test config")?
        } else if let Some(args) = self.init_args {
            args.into_bytes()
        } else if let Some(path) = self.init_args_file {
            std::fs::read(&path)
                .with_context(|| format!("read RedStone init args from {}", path.display()))?
        } else {
            anyhow::bail!("provide --prod, --test, --init-args, or --init-args-file");
        };

        Ok(registry_spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}
