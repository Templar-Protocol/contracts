use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use serde::Deserialize;
use templar_common::market::MarketConfiguration;
use templar_gateway_methods_spec::market as spec;
use templar_gateway_types::NearToken;

use crate::commands::full_access_key::FullAccessKeyArgs;
use crate::context::CliContext;

/// Deploy a market from a registered version, granting the signer a full access
/// key so the operator retains control of the new account.
#[derive(Args, Debug)]
pub struct Create {
    /// Registry that holds the market version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry (e.g. `usdc-near`).
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the market contract in the registry.
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// JSON file with the market init args (`{"configuration": ...}`).
    #[arg(long, value_name = "PATH")]
    init_args_file: std::path::PathBuf,
    #[command(flatten)]
    full_access_keys: FullAccessKeyArgs,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketInitArgs {
    configuration: MarketConfiguration,
}

impl Create {
    pub fn try_into_spec(self, ctx: &CliContext) -> anyhow::Result<spec::Create> {
        let full_access_keys = self.full_access_keys.resolve(ctx)?;
        let file = std::fs::File::open(&self.init_args_file).with_context(|| {
            format!(
                "open market init args from {}",
                self.init_args_file.display()
            )
        })?;
        let init_args: MarketInitArgs =
            serde_json::from_reader(file).context("parse market init args")?;

        Ok(spec::Create {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            configuration: init_args.configuration,
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}
