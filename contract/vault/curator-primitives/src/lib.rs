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
    cap_group::{
        can_allocate_to_group, compute_effective_cap, enforce_cap_group, CapGroup, CapGroupError,
        CapGroupId, CapGroupRecord,
    },
    market_lock::{is_market_locked, MarketLock, MarketLockSet},
    refresh_plan::{
        build_refresh_plan, compute_refresh_plan_total, validate_refresh_plan, RefreshPlan,
        RefreshPlanError,
    },
    supply_queue::{
        compute_queue_total, dequeue_supply, enqueue_supply, SupplyQueue, SupplyQueueEntry,
        SupplyQueueError,
    },
    withdraw_route::{
        build_withdraw_route, compute_route_total, validate_withdraw_route, WithdrawRoute,
        WithdrawRouteEntry, WithdrawRouteError,
    },
};

pub use recovery::{
    determine_recovery_action, handle_allocation_failure, handle_payout_failure,
    handle_refresh_failure, handle_withdrawal_failure, RecoveryAction, RecoveryContext,
    RecoveryError, RecoveryOutcome,
};
