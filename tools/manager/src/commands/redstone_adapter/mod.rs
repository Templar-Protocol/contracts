pub mod config;
pub mod deploy;
pub mod feed;
pub mod remove;
pub mod role;
pub mod update_prices;
pub mod write_prices;

use std::str::FromStr;

use templar_common::oracle::redstone::Role;

use crate::CliContext;

/// CLI-friendly mirror of [`Role`] that implements the traits clap needs.
#[derive(Debug, Clone)]
pub enum CliRole {
    ModifyRoles,
    TrustedUpdater,
}

impl FromStr for CliRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ModifyRoles" => Ok(Self::ModifyRoles),
            "TrustedUpdater" => Ok(Self::TrustedUpdater),
            _ => Err(format!(
                "unknown role: {s} (expected \"ModifyRoles\" or \"TrustedUpdater\")"
            )),
        }
    }
}

impl std::fmt::Display for CliRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModifyRoles => f.write_str("ModifyRoles"),
            Self::TrustedUpdater => f.write_str("TrustedUpdater"),
        }
    }
}

impl From<CliRole> for Role {
    fn from(cli: CliRole) -> Self {
        match cli {
            CliRole::ModifyRoles => Role::ModifyRoles,
            CliRole::TrustedUpdater => Role::TrustedUpdater,
        }
    }
}

#[derive(clap::Args)]
pub struct RedStoneAdapterArgs {
    #[command(subcommand)]
    command: RedStoneAdapterCommand,
}

#[derive(clap::Subcommand)]
enum RedStoneAdapterCommand {
    /// Deploy a RedStone adapter contract
    Deploy(deploy::DeployRedStoneAdapter),

    /// Delete a RedStone adapter account
    Remove(remove::RedStoneAdapterRemove),

    /// Query feed data from a RedStone adapter
    Feed(feed::FeedArgs),

    /// View the adapter configuration
    Config(config::AdapterConfig),

    /// Manage RBAC roles on a RedStone adapter
    Role(role::RoleArgs),

    /// Write prices to a RedStone adapter
    WritePrices(write_prices::WritePrices),

    /// Fetch live prices from RedStone and write them to an adapter
    UpdatePrices(update_prices::UpdatePrices),
}

impl RedStoneAdapterArgs {
    pub async fn run(self, ctx: &CliContext) -> anyhow::Result<()> {
        match self.command {
            RedStoneAdapterCommand::Deploy(a) => a.run(ctx).await,
            RedStoneAdapterCommand::Remove(a) => a.run(ctx).await,
            RedStoneAdapterCommand::Feed(a) => a.run(ctx).await,
            RedStoneAdapterCommand::Config(a) => a.run(ctx).await,
            RedStoneAdapterCommand::Role(a) => a.run(ctx).await,
            RedStoneAdapterCommand::WritePrices(a) => a.run(ctx).await,
            RedStoneAdapterCommand::UpdatePrices(a) => a.run(ctx).await,
        }
    }
}
