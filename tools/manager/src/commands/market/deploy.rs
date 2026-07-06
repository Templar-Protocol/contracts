use templar_common::market::MarketConfiguration;
use templar_contract_artifacts::ArtifactId;
use templar_gateway_types::MarketVersion;

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

    const ARTIFACT: ArtifactId = ArtifactId::Market;
}

impl DeployMarket {
    #[tracing::instrument(skip_all, name = "deploy_market")]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
