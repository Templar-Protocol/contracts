mod create;
mod get_config;
mod list_role;
mod read_price_data;
mod set_role;
mod update_prices;
mod write_prices;

pub use create::Create;
pub use get_config::GetConfig;
pub use list_role::ListRole;
pub use read_price_data::ReadPriceData;
pub use set_role::SetRole;
pub use update_prices::UpdatePrices;
pub use write_prices::WritePrices;

use clap::{Subcommand, ValueEnum};
use templar_gateway_methods_spec::redstone as spec;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum RedstoneNs {
    /// Deploy a RedStone adapter from a registry (with `--prod`/`--test` config
    /// presets).
    Create(Create),
    /// Read the RedStone adapter's configuration.
    GetConfig(GetConfig),
    /// Read stored price data for one or more feeds.
    ReadPriceData(ReadPriceData),
    /// List the accounts holding a role.
    ListRole(ListRole),
    /// Grant or revoke a role on an account.
    SetRole(SetRole),
    /// Write a signed RedStone payload on-chain.
    WritePrices(WritePrices),
    /// Fetch signed prices from the RedStone bridge and write them on-chain.
    UpdatePrices(UpdatePrices),
}

/// Shared `--role` value for the RedStone role commands.
#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RoleArg {
    ModifyRoles,
    TrustedUpdater,
}

impl From<RoleArg> for spec::RoleValue {
    fn from(arg: RoleArg) -> Self {
        match arg {
            RoleArg::ModifyRoles => Self::ModifyRoles,
            RoleArg::TrustedUpdater => Self::TrustedUpdater,
        }
    }
}
