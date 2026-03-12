pub mod clear_deployments;
pub mod deploy;
pub mod remove;
pub mod version;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct RegistryArgs {
    #[command(subcommand)]
    command: RegistryCommand,
}

#[derive(clap::Subcommand, Debug)]
enum RegistryCommand {
    /// Build and deploy the registry contract
    Deploy(deploy::DeployRegistry),

    /// Remove all versions from a registry then delete its account
    Remove(remove::RemoveRegistry),

    /// Manage versions stored in a registry
    Version(version::VersionArgs),

    /// Remove all markets listed in the registry's deployments
    ClearDeployments(clear_deployments::ClearDeployments),
}

impl RegistryArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            RegistryCommand::Deploy(a) => a.run(ctx).await,
            RegistryCommand::Remove(a) => a.run(ctx).await,
            RegistryCommand::Version(a) => a.run(ctx).await,
            RegistryCommand::ClearDeployments(a) => a.run(ctx).await,
        }
    }
}
