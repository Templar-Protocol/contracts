use anyhow::Context;
use templar_common::oracle::redstone::Config;

use crate::commands::{DeployFromRegistry, SignerArgs};

#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ConfigSource {
    /// Use the production RedStone configuration
    #[arg(long)]
    pub prod: bool,
    /// Use the test RedStone configuration
    #[arg(long)]
    pub test: bool,
    /// JSON-encoded RedStone `Config`
    #[arg(long)]
    pub configuration: Option<serde_json::Value>,
}

impl ConfigSource {
    pub fn resolve(&self) -> anyhow::Result<Config> {
        if self.prod {
            Ok(templar_common::oracle::redstone::config::prod())
        } else if self.test {
            Ok(templar_common::oracle::redstone::config::test())
        } else if let Some(configuration) = self.configuration.clone() {
            serde_json::from_value(configuration).context("invalid RedStone configuration")
        } else {
            // Guaranteed by #[group(...)] attribute on ConfigSource struct
            unreachable!()
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
        let config = self.config_source.resolve()?;
        let init_args = serde_json::to_vec(&serde_json::json!({ "config": config }))
            .context("serialise init args")?;

        tracing::info!("Creating RedStone adapter from registry");
        self.deploy
            .run(ctx, &self.signer.signer(), init_args)
            .await?;
        tracing::info!("RedStone adapter created");

        Ok(())
    }
}
