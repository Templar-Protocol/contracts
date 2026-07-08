use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use serde::Deserialize;
use templar_common::market::MarketConfiguration;
use templar_gateway_methods_spec::market as spec;
use templar_gateway_types::NearToken;

#[derive(Args, Debug)]
pub struct Create {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    #[arg(long, value_name = "PATH")]
    init_args_file: std::path::PathBuf,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketInitArgs {
    configuration: MarketConfiguration,
}

impl Create {
    pub fn parse(self) -> anyhow::Result<spec::Create> {
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
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}
