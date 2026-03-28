use crate::commands::deployment::StandardDeploy;

const REGISTRY_PACKAGE: &str = "templar-registry-contract";

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RegistryInitArgs {}

#[derive(clap::Args, Debug)]
pub struct DeployRegistry {
    #[command(flatten)]
    pub deploy: StandardDeploy,
}

impl DeployRegistry {
    #[tracing::instrument(skip_all, name = "deploy_registry")]
    pub async fn run(self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        self.deploy
            .run::<RegistryInitArgs>(ctx, REGISTRY_PACKAGE)
            .await
    }
}
