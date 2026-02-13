//! Policy module bridging curator-primitives with Soroban vault types.
//!
//! This module provides:
//! - Re-exports of curator-primitives types for policy enforcement
//! - Soroban-specific helpers for market lock operations
//! - Type aliases for Soroban market identifiers

pub use templar_curator_primitives::policy::lock_filter::{
    build_allocation_plan_with_locks, build_refresh_plan_with_locks,
    build_withdrawal_plan_with_locks, filter_allocation_plan, filter_unlocked_targets,
};

pub use templar_curator_primitives::policy::{
    cap_group::{CapGroup, CapGroupError, CapGroupId, CapGroupRecord},
    market_lock::{validate_lock_expiry, MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

#[cfg(test)]
mod tests;
