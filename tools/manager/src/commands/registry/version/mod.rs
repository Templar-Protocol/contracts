pub mod add;
pub mod list;
pub mod remove;

use crate::CliContext;

#[derive(clap::Args)]
pub struct VersionArgs {
    #[command(subcommand)]
    command: VersionCommand,
}

#[derive(clap::Subcommand)]
enum VersionCommand {
    /// Add a contract version to the registry
    Add(add::AddVersion),

    /// List all versions registered in the registry
    List(list::ListVersions),

    /// Remove one or all versions from the registry
    Remove(remove::VersionRemove),
}

impl VersionArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            VersionCommand::Add(a) => a.run(ctx).await,
            VersionCommand::List(a) => a.run(ctx).await,
            VersionCommand::Remove(a) => a.run(ctx).await,
        }
    }
}
