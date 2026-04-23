#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod contract;
mod error;

pub use contract::SorobanCuratorProxyContract;
pub use error::ContractError;
pub use templar_soroban_governance::{
    GovernanceActionKind, PendingProposal, TimelockKind, Timelocks,
};
pub use templar_soroban_shared_types::VaultCommandResult;

#[cfg(test)]
mod tests;
