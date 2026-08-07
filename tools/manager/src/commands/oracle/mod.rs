mod update_lazer;
mod update_prices;
mod update_pyth;
mod update_red_stone;

pub use update_lazer::UpdateLazer;
pub use update_prices::UpdatePrices;
pub use update_pyth::UpdatePyth;
pub use update_red_stone::UpdateRedStone;

use clap::Subcommand;

/// The gateway's `oracle.*` update methods. Each fetches its payload inside the
/// gateway, so a subcommand carries only the source flags its own method reaches:
/// only `update-lazer` and `update-prices` require `--pyth-lazer-api-key`.
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum OracleNs {
    /// Fetch a Pyth VAA from Hermes and write it to a Pyth oracle.
    #[command(name = "update-pyth")]
    Pyth(UpdatePyth),
    /// Fetch signed prices from the RedStone bridge and write them to a RedStone adapter.
    #[command(name = "update-red-stone")]
    RedStone(UpdateRedStone),
    /// Fetch a signed Pyth Lazer payload and write it to a Lazer adapter.
    #[command(name = "update-lazer")]
    Lazer(UpdateLazer),
    /// Resolve an oracle's price dependencies and submit every underlying update.
    #[command(name = "update-prices")]
    Prices(UpdatePrices),
}
