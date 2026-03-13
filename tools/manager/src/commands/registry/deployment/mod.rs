pub mod clear;
pub mod list;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct DeploymentArgs {
    #[command(subcommand)]
    command: DeploymentCommand,
}

#[derive(clap::Subcommand, Debug)]
enum DeploymentCommand {
    /// List all markets deployed from the registry
    List(list::ListDeployments),

    /// Remove all markets deployed from the registry
    Clear(clear::ClearDeployments),
}

impl DeploymentArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            DeploymentCommand::List(a) => a.run(ctx).await,
            DeploymentCommand::Clear(a) => a.run(ctx).await,
        }
    }
}
