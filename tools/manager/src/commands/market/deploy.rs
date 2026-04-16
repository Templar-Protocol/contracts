use blockchain_gateway_core::MarketVersion;
use templar_common::market::MarketConfiguration;

use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::GeneralArgsLoader,
    Runner,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct MarketInitArgs {
    pub configuration: MarketConfiguration,
}

#[derive(clap::Args)]
pub struct DeployMarket {
    #[command(subcommand)]
    pub deploy: Deploy<Self>,
}

impl DeploymentSpec for DeployMarket {
    type Args = MarketInitArgs;
    type ArgsLoader = GeneralArgsLoader;
    type Version = MarketVersion;

    const PACKAGE_ID: &'static str = "templar-market-contract";
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "deploy_market")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
