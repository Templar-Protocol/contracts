//! Chain-agnostic curator primitives for Templar Protocol vaults.
//!
//! This crate provides shared curator policy and recovery logic that can be used
//! by both NEAR and Soroban vault executors. It depends only on `templar-vault-kernel`
//! types and contains no chain-specific SDK dependencies.
//!
//! # Modules
//!
//! - [`policy`]: Cap groups, supply queues, withdraw routes, refresh plans, and market locks
//! - [`recovery`]: Recovery action determination and state machine recovery logic
//!
//! # Design Principles
//!
//! 1. **Chain-agnostic**: All types work without any chain SDK dependencies
//! 2. **Pure functions**: No side effects, no storage access
//! 3. **Defensive math**: All calculations use saturating arithmetic to prevent overflow

#![no_std]

extern crate alloc;

pub mod policy;
pub mod recovery;

#[cfg(test)]
mod golden_tests;

// Re-exports for convenience
pub use policy::{
    cap_group::{CapGroup, CapGroupError, CapGroupId, CapGroupRecord},
    cooldown::{Cooldown, CooldownError},
    market_lock::{MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

pub use recovery::{
    determine_recovery_action, handle_allocation_failure, handle_payout_failure,
    handle_payout_failure_default, handle_refresh_failure, handle_withdrawal_failure,
    RecoveryOutcome,
};
