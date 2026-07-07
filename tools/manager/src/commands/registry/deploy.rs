use templar_contract_artifacts::ArtifactId;
use templar_gateway_types::RegistryVersion;

use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::EmptyArgsLoader,
    Runner,
};

#[derive(clap::Args)]
pub struct DeployRegistry {
    #[command(subcommand)]
    pub deploy: Deploy<Self>,
}

impl DeploymentSpec for DeployRegistry {
    type Args = ();
    type ArgsLoader = EmptyArgsLoader;
    type Version = RegistryVersion;

    const ARTIFACT: ArtifactId = ArtifactId::Registry;
}

impl DeployRegistry {
    #[tracing::instrument(skip_all, name = "deploy_registry")]
    pub async fn run(self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
