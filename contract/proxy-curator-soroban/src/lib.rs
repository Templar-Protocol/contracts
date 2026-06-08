#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod contract;
mod error;

pub use contract::{
    AllocationDelta, CapGroupUpdate, CapGroupUpdateKey, Fees, GovernanceView, Restrictions,
    SorobanCuratorProxyContract, VaultPreview, VaultView,
};
pub use error::ContractError;
pub use templar_soroban_governance::{
    GovernanceActionKind, PendingProposal, SupplyQueueProposalEntry, TimelockKind, Timelocks,
};

#[cfg(test)]
mod tests;
