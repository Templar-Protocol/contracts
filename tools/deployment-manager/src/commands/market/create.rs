use anyhow::Context;
use serde_json::json;
use templar_common::market::MarketConfiguration;

use crate::commands::{DeployFromRegistry, SignerArgs};

#[derive(clap::Args, Debug)]
pub struct CreateMarket {
    #[command(flatten)]
    signer: SignerArgs,
    #[command(flatten)]
    deploy: DeployFromRegistry,
    /// JSON-encoded `MarketConfiguration`
    #[arg(long)]
    configuration: serde_json::Value,
    /// Skip validation of --configuration
    #[arg(long)]
    skip_validation: bool,
}

impl CreateMarket {
    #[tracing::instrument(skip_all, name = "market_create")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        if !self.skip_validation {
            let configuration: MarketConfiguration =
                serde_json::from_value(self.configuration.clone())
                    .context("invalid market configuration")?;

            configuration
                .validate()
                .context("market configuration validation failed")?;
        }

        let init_args = serde_json::to_vec(&json!({ "configuration": self.configuration }))?;

        tracing::info!("Creating market from registry");
        self.deploy
            .run(ctx, &self.signer.signer(), init_args)
            .await?;
        tracing::info!("Market created");

        Ok(())
    }
}
