//! Policy module bridging curator-primitives with Soroban vault types.
//!
//! This module provides:
//! - Re-exports of curator-primitives types for policy enforcement
//! - Soroban-specific helpers for market lock operations
//! - Type aliases for Soroban market identifiers

use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

// Re-export curator-primitives types for external consumers
pub use templar_curator_primitives::policy::{
    cap_group::{CapGroup, CapGroupError, CapGroupId, CapGroupRecord},
    market_lock::{MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan, RefreshPlanError},
    state::{MarketConfig, PolicyState},
    supply_queue::{SupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry, WithdrawRouteError},
};

/// Filter a list of targets to exclude locked markets.
///
/// # Arguments
/// * `lock_set` - The set of active locks
/// * `targets` - List of targets to filter
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// List of targets that are NOT locked.
pub fn filter_unlocked_targets(
    lock_set: &MarketLockSet,
    targets: &[TargetId],
    current_ns: u64,
) -> Vec<TargetId> {
    targets
        .iter()
        .filter(|t| !lock_set.is_locked(**t, current_ns))
        .copied()
        .collect()
}

/// Build an allocation plan excluding locked markets.
///
/// Takes a supply queue and filters out any entries for locked markets,
/// then converts to an allocation plan.
///
/// # Arguments
/// * `queue` - The supply queue
/// * `lock_set` - The set of active locks
/// * `current_ns` - Current timestamp
///
/// # Returns
/// Allocation plan as (TargetId, amount) pairs.
pub fn build_allocation_plan_with_locks(
    queue: &SupplyQueue,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    // Filter queue to exclude locked markets
    let filtered: SupplyQueue = queue
        .entries
        .iter()
        .filter(|e| !lock_set.is_locked(e.target_id, current_ns))
        .cloned()
        .collect::<Vec<_>>()
        .into();

    filtered.to_allocation_plan()
}

/// Build a withdrawal plan excluding locked markets.
///
/// # Arguments
/// * `route` - The withdrawal route
/// * `lock_set` - The set of active locks
/// * `current_ns` - Current timestamp
///
/// # Returns
/// Withdrawal plan excluding locked markets.
pub fn build_withdrawal_plan_with_locks(
    route: &WithdrawRoute,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    let filtered_entries: Vec<WithdrawRouteEntry> = route
        .entries
        .iter()
        .filter(|e| !lock_set.is_locked(e.target_id, current_ns))
        .cloned()
        .collect();

    let filtered_route = WithdrawRoute::from_entries(filtered_entries, route.target_amount);

    filtered_route.to_withdrawal_plan()
}

/// Build a refresh plan excluding locked markets.
///
/// # Arguments
/// * `targets` - Target IDs to potentially refresh
/// * `lock_set` - The set of active locks
/// * `current_ns` - Current timestamp
///
/// # Returns
/// List of unlocked targets to refresh.
pub fn build_refresh_plan_with_locks(
    targets: &[TargetId],
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<TargetId> {
    filter_unlocked_targets(lock_set, targets, current_ns)
}

/// Filter an allocation plan to exclude locked markets.
///
/// This takes a raw plan (as passed to `begin_allocating`) and removes
/// any entries targeting locked markets.
///
/// # Arguments
/// * `plan` - Allocation plan as (TargetId, amount) pairs
/// * `lock_set` - The set of active locks
/// * `current_ns` - Current timestamp for expiry checking
///
/// # Returns
/// Filtered plan with locked targets removed.
pub fn filter_allocation_plan(
    plan: &[(TargetId, u128)],
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    plan.iter()
        .filter(|(target_id, _)| !lock_set.is_locked(*target_id, current_ns))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_filter_unlocked_targets() {
        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);
        set = set.acquire(lock, 1000).unwrap();

        let targets = vec![1, 2, 3, 4];
        let unlocked = filter_unlocked_targets(&set, &targets, 1500);

        assert_eq!(unlocked, vec![1, 3, 4]);
        assert!(!unlocked.contains(&2)); // locked
    }

    #[test]
    fn test_filter_unlocked_after_expiry() {
        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000).with_expiry(2000); // expires at 2000
        set = set.acquire(lock, 1000).unwrap();

        let targets = vec![1, 2, 3];

        // Before expiry
        let unlocked = filter_unlocked_targets(&set, &targets, 1500);
        assert_eq!(unlocked, vec![1, 3]);

        // After expiry
        let unlocked = filter_unlocked_targets(&set, &targets, 2500);
        assert_eq!(unlocked, vec![1, 2, 3]); // 2 is now unlocked
    }

    #[test]
    fn test_build_allocation_plan_with_locks() {
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

        // Target 2 should be excluded
        assert!(plan.iter().all(|(t, _)| *t != 2));
        // Should include targets 1 and 3
        assert!(plan.iter().any(|(t, _)| *t == 1));
        assert!(plan.iter().any(|(t, _)| *t == 3));
    }

    #[test]
    fn test_build_withdrawal_plan_with_locks() {
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

        // Target 1 should be excluded
        assert!(plan.iter().all(|(t, _)| *t != 1));
        // Should include targets 2 and 3
        assert!(plan.iter().any(|(t, _)| *t == 2) || plan.iter().any(|(t, _)| *t == 3));
    }

    #[test]
    fn test_build_refresh_plan_with_locks() {
        let targets = vec![1, 2, 3, 4, 5];

        let mut set = MarketLockSet::new();
        set = set.acquire(MarketLock::new(2, 1000), 1000).unwrap();
        set = set.acquire(MarketLock::new(4, 1000), 1000).unwrap();

        let plan = build_refresh_plan_with_locks(&targets, &set, 1500);

        assert_eq!(plan, vec![1, 3, 5]);
        assert!(!plan.contains(&2));
        assert!(!plan.contains(&4));
    }

    #[test]
    fn test_empty_lock_set_passes_all() {
        let set = MarketLockSet::new();
        let targets = vec![1, 2, 3, 4, 5];

        let unlocked = filter_unlocked_targets(&set, &targets, 1000);

        assert_eq!(unlocked, targets);
    }

    #[test]
    fn test_all_locked_returns_empty() {
        let mut set = MarketLockSet::new();
        set = set.acquire(MarketLock::new(1, 1000), 1000).unwrap();
        set = set.acquire(MarketLock::new(2, 1000), 1000).unwrap();

        let targets = vec![1, 2];
        let unlocked = filter_unlocked_targets(&set, &targets, 1500);

        assert!(unlocked.is_empty());
    }

    #[test]
    fn test_filter_allocation_plan() {
        let plan = vec![(1, 100), (2, 200), (3, 300)];

        let mut set = MarketLockSet::new();
        let lock = MarketLock::new(2, 1000);
        set = set.acquire(lock, 1000).unwrap();

        let filtered = super::filter_allocation_plan(&plan, &set, 1500);

        // Target 2 should be excluded
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|(t, _)| *t != 2));
        // Should include targets 1 and 3 with original amounts
        assert!(filtered.contains(&(1, 100)));
        assert!(filtered.contains(&(3, 300)));
    }

    #[test]
    fn test_filter_allocation_plan_empty_locks() {
        let plan = vec![(1, 100), (2, 200)];
        let set = MarketLockSet::new();

        let filtered = super::filter_allocation_plan(&plan, &set, 1000);

        // All should pass through
        assert_eq!(filtered, plan);
    }

    #[test]
    fn test_filter_allocation_plan_all_locked() {
        let plan = vec![(1, 100), (2, 200)];

        let mut set = MarketLockSet::new();
        set = set.acquire(MarketLock::new(1, 1000), 1000).unwrap();
        set = set.acquire(MarketLock::new(2, 1000), 1000).unwrap();

        let filtered = super::filter_allocation_plan(&plan, &set, 1500);

        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_allocation_plan_respects_expiry() {
        let plan = vec![(1, 100), (2, 200)];

        let mut set = MarketLockSet::new();
        // Lock expires at 2000
        let lock = MarketLock::new(1, 1000).with_expiry(2000);
        set = set.acquire(lock, 1000).unwrap();

        // Before expiry - target 1 should be filtered
        let filtered_before = super::filter_allocation_plan(&plan, &set, 1500);
        assert_eq!(filtered_before.len(), 1);
        assert_eq!(filtered_before[0], (2, 200));

        // After expiry - all should pass
        let filtered_after = super::filter_allocation_plan(&plan, &set, 2500);
        assert_eq!(filtered_after.len(), 2);
    }
}
