mod create;
mod remove;

pub use create::Create;
pub use remove::Remove;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum MarketNs {
    Create(Create),
    /// Remove a market: recover its assets to a beneficiary, then delete the
    /// (signer) account.
    Remove(Remove),
}
