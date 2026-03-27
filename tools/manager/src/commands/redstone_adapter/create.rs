use std::path::PathBuf;

use anyhow::Context;
use templar_common::oracle::redstone::Config;

use crate::commands::{json_input::JsonSource, DeployFromRegistry, SignerArgs};

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
    pub configuration: Option<String>,
    /// Path to a JSON configuration file
    #[arg(long)]
    pub configuration_file: Option<PathBuf>,
}

impl ConfigSource {
    pub fn init_args(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec(&serde_json::json!({ "config": self.resolve()? }))
            .context("serialise init args")
    }

    fn resolve(&self) -> anyhow::Result<Config> {
        if self.prod {
            Ok(templar_common::oracle::redstone::config::prod())
        } else if self.test {
            Ok(templar_common::oracle::redstone::config::test())
        } else {
            JsonSource::new(
                self.configuration.as_deref(),
                self.configuration_file.as_deref(),
            )?
            .parse()
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct CreateRedStoneAdapter {
    #[command(flatten)]
    pub signer: SignerArgs,

    #[command(flatten)]
    pub deploy: DeployFromRegistry,

    #[command(flatten)]
    pub config_source: ConfigSource,
}

impl CreateRedStoneAdapter {
    #[tracing::instrument(skip_all, name = "redstone_adapter_create")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let init_args = self.config_source.init_args()?;

        tracing::info!("Creating RedStone adapter from registry");
        self.deploy
            .run(ctx, &self.signer.signer(), init_args)
            .await?;
        tracing::info!("RedStone adapter created");

        Ok(())
    }
}
