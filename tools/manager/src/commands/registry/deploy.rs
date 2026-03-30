use templar_tools_common::version::RegistryVersion;

use crate::{
    commands::deployment::{Deploy, DeploymentSpec},
    util::EmptyArgsProvider,
    Runner,
};

#[derive(clap::Args)]
pub struct DeployRegistry {
    #[command(subcommand)]
    pub deploy: Deploy<Self>,
}

impl DeploymentSpec for DeployRegistry {
    type Args = ();
    type ArgsArgs = EmptyArgsProvider;
    type Version = RegistryVersion;

    const PACKAGE_ID: &'static str = "templar-registry-contract";
}

impl DeployRegistry {
    #[tracing::instrument(skip_all, name = "deploy_registry")]
    pub async fn run(self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy.run(ctx, &()).await
    }
}
