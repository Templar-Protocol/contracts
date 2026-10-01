mod accumulate;
mod get;
mod harvest;
mod withdraw;

pub use accumulate::Accumulate;
pub use get::Get;
pub use harvest::Harvest;
pub use withdraw::Withdraw;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum StaticYieldNs {
    /// Read an account's accumulated static yield.
    Get(Get),
    /// Accumulate an account's static yield up to the latest snapshot.
    Accumulate(Accumulate),
    /// Withdraw the signer's accumulated static yield.
    Withdraw(Withdraw),
    /// Accumulate and withdraw the signer's static yield across markets,
    /// optionally forwarding the proceeds.
    Harvest(Harvest),
}
