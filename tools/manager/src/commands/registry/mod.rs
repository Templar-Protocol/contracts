pub mod deploy;
pub mod deployment;
pub mod remove;
pub mod version;

use crate::CliContext;

#[derive(clap::Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    command: RegistryCommand,
}

#[derive(clap::Subcommand)]
enum RegistryCommand {
    /// Build and deploy the registry contract
    Deploy(deploy::DeployRegistry),

    /// Remove all versions from a registry then delete its account
    Remove(remove::RemoveRegistry),

    /// Manage versions stored in a registry
    Version(version::VersionArgs),

    /// Manage deployments tracked by a registry
    Deployment(deployment::DeploymentArgs),
}

impl RegistryArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            RegistryCommand::Deploy(a) => a.run(ctx).await,
            RegistryCommand::Remove(a) => a.run(ctx).await,
            RegistryCommand::Version(a) => a.run(ctx).await,
            RegistryCommand::Deployment(a) => a.run(ctx).await,
        }
    }
}
