use anyhow::Context;
use serde_json::json;
use templar_common::market::MarketConfiguration;

use crate::commands::{DeployFromRegistry, SignerArgs};

#[derive(clap::Args, Debug)]
pub struct CreateMarket {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[command(flatten)]
    pub deploy: DeployFromRegistry,
    /// JSON-encoded `MarketConfiguration`
    #[arg(long)]
    pub configuration: String,
}

impl CreateMarket {
    #[tracing::instrument(skip_all, name = "market_create")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let configuration: MarketConfiguration =
            serde_json::from_str(&self.configuration).context("invalid market configuration")?;

        configuration
            .validate()
            .context("market configuration validation failed")?;

        let init_args = serde_json::to_vec(&json!({ "configuration": configuration }))?;

        tracing::info!("Creating market from registry");
        self.deploy
            .run(ctx, &self.signer.signer(), init_args)
            .await?;
        tracing::info!("Market created");

        Ok(())
    }
}
