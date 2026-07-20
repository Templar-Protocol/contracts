mod accept;
mod get;
mod get_proposed;
mod propose;
mod renounce;

pub use accept::Accept;
pub use get::Get;
pub use get_proposed::GetProposed;
pub use propose::Propose;
pub use renounce::Renounce;

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum OwnerNs {
    /// Read the current contract owner.
    Get(Get),
    /// Read the pending proposed owner.
    GetProposed(GetProposed),
    /// Propose a new owner, starting a two-step ownership transfer.
    Propose(Propose),
    /// Accept a pending ownership transfer.
    Accept(Accept),
    /// Renounce contract ownership.
    Renounce(Renounce),
}
