mod create;
mod export;
mod remove;

pub use create::Create;
pub use export::Export;
pub use remove::Remove;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum MarketNs {
    /// Deploy a market from a registered version.
    Create(Create),
    /// Reconstruct a deployment spec from a deployed market.
    Export(Export),
    /// Remove a market: recover its assets to a beneficiary, then delete the
    /// (signer) account.
    Remove(Remove),
}
