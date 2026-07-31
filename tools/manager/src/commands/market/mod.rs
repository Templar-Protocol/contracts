mod create;
mod export;
mod plan;
mod remove;

pub use create::Create;
pub use export::Export;
pub use plan::{Apply, Plan};
pub use remove::Remove;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum MarketNs {
    /// Deploy a market from a registered version.
    Create(Create),
    /// Reconstruct a deployment spec from a deployed market.
    Export(Export),
    /// Generate a deployment from a spec as an editable plan file.
    Plan(Plan),
    /// Send a plan file produced by `market plan`.
    Apply(Apply),
    /// Remove a market: recover its assets to a beneficiary, then delete the
    /// (signer) account.
    Remove(Remove),
}
