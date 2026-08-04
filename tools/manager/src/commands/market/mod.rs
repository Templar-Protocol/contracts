mod create;
mod export;
mod plan;
mod remove;
mod verify;

pub use create::Create;
pub use export::Export;
pub use plan::{Apply, Plan};
pub use remove::Remove;
pub use verify::Verify;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum MarketNs {
    /// Deploy a market from a registered version.
    Create(Create),
    /// Reconstruct a deployment spec from a deployed market.
    Export(Export),
    /// Generate a deployment from a spec as a reviewable plan file.
    Plan(Plan),
    /// Send a plan file produced by `market plan`.
    Apply(Apply),
    /// Re-run the preflight against a market that already exists.
    Verify(Verify),
    /// Remove a market: recover its assets to a beneficiary, then delete the
    /// (signer) account.
    Remove(Remove),
}
