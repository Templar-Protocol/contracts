pub mod get;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct FeedArgs {
    #[command(subcommand)]
    command: FeedCommand,
}

#[derive(clap::Subcommand, Debug)]
enum FeedCommand {
    /// Get price data for one or more feeds
    Get(get::FeedGet),
}

impl FeedArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            FeedCommand::Get(a) => a.run(ctx).await,
        }
    }
}
