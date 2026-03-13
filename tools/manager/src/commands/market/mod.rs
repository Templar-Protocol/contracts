pub mod create;
pub mod deploy;
pub mod remove;

use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct MarketArgs {
    #[command(subcommand)]
    command: MarketCommand,
}

#[derive(clap::Subcommand, Debug)]
enum MarketCommand {
    /// Create a new market via the registry
    Create(create::CreateMarket),

    /// Deploy the market contract directly onto an account
    Deploy(deploy::DeployMarket),

    /// Remove a market: recover NEP-141 tokens then delete the account
    Remove(remove::MarketRemove),
}

impl MarketArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            MarketCommand::Create(a) => a.run(ctx).await,
            MarketCommand::Deploy(a) => a.run(ctx).await,
            MarketCommand::Remove(a) => a.run(ctx).await,
        }
    }
}
