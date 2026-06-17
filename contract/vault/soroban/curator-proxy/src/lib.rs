#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod contract;
mod error;
mod governance_abi;

pub use contract::{
    AllocationDelta, CapGroupUpdate, CapGroupUpdateKey, Fees, GovernanceView, Restrictions,
    SorobanCuratorProxyContract, VaultPreview, VaultView,
};
pub use error::ContractError;
pub use governance_abi::{
    GovernanceAction, GovernanceActionKind, PendingProposal, SupplyQueueProposalEntry,
    TimelockKind, Timelocks,
};

#[cfg(test)]
mod tests;
