pub mod add;
pub mod remove;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct VersionArgs {
    #[command(subcommand)]
    command: VersionCommand,
}

#[derive(clap::Subcommand, Debug)]
enum VersionCommand {
    /// Add a contract version to the registry
    Add(add::AddVersion),

    /// Remove one or all versions from the registry
    Remove(remove::VersionRemove),
}

impl VersionArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            VersionCommand::Add(a) => a.run(ctx).await,
            VersionCommand::Remove(a) => a.run(ctx).await,
        }
    }
}
