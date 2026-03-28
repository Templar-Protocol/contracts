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
    /// Deploy a market contract
    Deploy(deploy::DeployMarket),

    /// Remove a market: recover NEP-141 tokens then delete the account
    Remove(remove::MarketRemove),
}

impl MarketArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            MarketCommand::Deploy(a) => a.run(ctx).await,
            MarketCommand::Remove(a) => a.run(ctx).await,
        }
    }
}
