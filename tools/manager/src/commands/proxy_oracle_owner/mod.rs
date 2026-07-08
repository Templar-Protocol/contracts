mod accept_owner;
mod get_owner;
mod get_proposed_owner;
mod propose_owner;
mod renounce_owner;

pub use accept_owner::AcceptOwner;
pub use get_owner::GetOwner;
pub use get_proposed_owner::GetProposedOwner;
pub use propose_owner::ProposeOwner;
pub use renounce_owner::RenounceOwner;

use clap::Subcommand;

#[allow(clippy::enum_variant_names)] // these are the contract's own owner ops
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleOwnerNs {
    /// Read the current owner of a proxy-oracle account.
    GetOwner(GetOwner),
    /// Read the pending proposed owner of a proxy-oracle account.
    GetProposedOwner(GetProposedOwner),
    /// Propose a new owner, starting a two-step ownership transfer.
    ProposeOwner(ProposeOwner),
    /// Accept a pending ownership transfer.
    AcceptOwner(AcceptOwner),
    /// Renounce ownership of a proxy-oracle account.
    RenounceOwner(RenounceOwner),
}
