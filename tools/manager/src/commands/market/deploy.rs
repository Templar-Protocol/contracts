use templar_common::market::MarketConfiguration;

use crate::commands::deployment::StandardDeploy;

const MARKET_PACKAGE: &str = "templar-market-contract";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct MarketInitArgs {
    pub configuration: MarketConfiguration,
}

#[derive(clap::Args, Debug)]
pub struct DeployMarket {
    #[command(flatten)]
    pub deploy: StandardDeploy,
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "deploy_market")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run::<MarketInitArgs>(ctx, MARKET_PACKAGE).await
    }
}
