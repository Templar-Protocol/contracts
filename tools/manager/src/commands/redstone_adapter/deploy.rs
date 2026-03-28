use std::path::PathBuf;

use anyhow::Context;
use templar_common::oracle::redstone::Config;

use crate::commands::{deployment::Channel, json_input::JsonSource, SignerArgs};

const REDSTONE_ADAPTER_PACKAGE: &str = "templar-redstone-adapter-contract";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RedstoneAdapterInitArgs {
    pub config: Config,
}

#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ConfigSource {
    /// Use the production RedStone configuration
    #[arg(long)]
    pub prod: bool,
    /// Use the test RedStone configuration
    #[arg(long)]
    pub test: bool,
    /// JSON configuration
    #[arg(long)]
    pub args: Option<String>,
    /// Path to a JSON configuration file
    #[arg(long)]
    pub args_file: Option<PathBuf>,
}

impl ConfigSource {
    pub fn prod() -> Self {
        Self {
            prod: true,
            test: false,
            args: None,
            args_file: None,
        }
    }

    pub fn test() -> Self {
        Self {
            prod: false,
            test: true,
            args: None,
            args_file: None,
        }
    }

    pub fn inline(args: String) -> Self {
        Self {
            prod: false,
            test: false,
            args: Some(args),
            args_file: None,
        }
    }

    pub fn from_file(args_file: PathBuf) -> Self {
        Self {
            prod: false,
            test: false,
            args: None,
            args_file: Some(args_file),
        }
    }

    pub fn from_config(config: Config) -> anyhow::Result<Self> {
        Ok(Self::inline(serde_json::to_string(
            &RedstoneAdapterInitArgs { config },
        )?))
    }

    pub fn load_vec(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec(&self.resolve()?).context("serialise init args")
    }

    fn resolve(&self) -> anyhow::Result<RedstoneAdapterInitArgs> {
        if self.prod {
            let config = templar_common::oracle::redstone::config::prod();
            Ok(RedstoneAdapterInitArgs { config })
        } else if self.test {
            let config = templar_common::oracle::redstone::config::test();
            Ok(RedstoneAdapterInitArgs { config })
        } else {
            JsonSource::new(self.args.as_deref(), self.args_file.as_deref())?.parse()
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct DeployRedStoneAdapter {
    #[command(flatten)]
    pub signer: SignerArgs,

    #[command(subcommand)]
    pub channel: Channel,

    #[command(flatten)]
    pub config_source: ConfigSource,
}

impl DeployRedStoneAdapter {
    #[tracing::instrument(skip_all, name = "deploy_redstone_adapter")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.channel
            .run(
                ctx,
                &self.signer,
                REDSTONE_ADAPTER_PACKAGE,
                self.config_source.load_vec()?,
            )
            .await
    }
}
