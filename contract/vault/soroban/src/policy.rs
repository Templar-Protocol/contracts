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

// Re-export curator-primitives types for external consumers
pub use templar_curator_primitives::policy::{
    cap_group::{CapGroup, CapGroupError, CapGroupId, CapGroupRecord},
    market_lock::{MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn reexport_filter_unlocked_targets_works() {
        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);
        set = set.acquire(lock, 1000).unwrap();

        let targets = vec![1, 2, 3];
        let unlocked = filter_unlocked_targets(&set, &targets, 1500);

        assert_eq!(unlocked, vec![1, 3]);
    }

    #[test]
    fn reexport_build_allocation_plan_with_locks_works() {
        let queue: SupplyQueue = vec![
            SupplyQueueEntry::new(1, 100),
            SupplyQueueEntry::new(2, 200),
            SupplyQueueEntry::new(3, 300),
        ]
        .into();

        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);
        set = set.acquire(lock, 1000).unwrap();

        let plan = build_allocation_plan_with_locks(&queue, &set, 1500);

        assert_eq!(plan, vec![(1, 100), (3, 300)]);
    }

    #[test]
    fn reexport_build_withdrawal_plan_with_locks_works() {
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(2, 200),
                WithdrawRouteEntry::new(3, 300),
            ],
            400, // target_amount
        );

        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(1, 1000);
        set = set.acquire(lock, 1000).unwrap();

        let plan = build_withdrawal_plan_with_locks(&route, &set, 1500);

        assert_eq!(plan, vec![(2, 200), (3, 300)]);
    }
}
