#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod contract;
mod error;

pub use contract::{
    AllocationDelta, GovernanceView, SorobanCuratorProxyContract, VaultPreview, VaultView,
};
pub use error::ContractError;
pub use templar_soroban_governance::{
    CapGroupUpdate, CapGroupUpdateKey, Fees, GovernanceActionKind, PendingProposal, Restrictions,
    TimelockKind, Timelocks,
};
pub use templar_soroban_shared_types::VaultCommandResult;

#[cfg(test)]
mod tests;
