//! Shared helpers for filtering locked targets from plans.

use alloc::{collections::VecDeque, vec::Vec};

use templar_vault_kernel::TargetId;

use super::{
    market_lock::MarketLockSet,
    supply_queue::{SupplyQueue, SupplyQueueEntry},
    withdraw_route::{WithdrawRoute, WithdrawRouteEntry},
};

/// Filter a target list to only unlocked targets.
#[must_use]
pub fn filter_unlocked_targets(
    lock_set: &MarketLockSet,
    targets: &[TargetId],
    current_ns: u64,
) -> Vec<TargetId> {
    targets
        .iter()
        .copied()
        .filter(|target| !lock_set.is_locked(*target, current_ns))
        .collect()
}

/// Filter an allocation plan to only unlocked targets.
#[must_use]
pub fn filter_allocation_plan(
    plan: &[(TargetId, u128)],
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    plan.iter()
        .copied()
        .filter(|(target_id, _)| !lock_set.is_locked(*target_id, current_ns))
        .collect()
}

/// Filter a supply queue to only unlocked targets.
#[must_use]
pub fn filter_supply_queue(
    queue: &SupplyQueue,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> SupplyQueue {
    let entries: VecDeque<SupplyQueueEntry> = queue
        .entries
        .iter()
        .filter(|entry| !lock_set.is_locked(entry.target_id, current_ns))
        .cloned()
        .collect();

    SupplyQueue {
        entries,
        max_length: queue.max_length,
    }
}

/// Filter a withdraw route to only unlocked targets.
#[must_use]
pub fn filter_withdraw_route(
    route: &WithdrawRoute,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> WithdrawRoute {
    let entries: Vec<WithdrawRouteEntry> = route
        .entries
        .iter()
        .filter(|entry| !lock_set.is_locked(entry.target_id, current_ns))
        .cloned()
        .collect();

    WithdrawRoute::from_entries(entries, route.target_amount)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::policy::market_lock::MarketLock;

    fn lock_set_with_target(target_id: TargetId) -> MarketLockSet {
        MarketLockSet::new()
            .acquire(MarketLock::new(target_id, 1_000), 1_000)
            .expect("lock should be acquirable")
    }

    #[test]
    fn filters_unlocked_targets() {
        let lock_set = lock_set_with_target(2);
        let targets = vec![1, 2, 3];
        assert_eq!(
            filter_unlocked_targets(&lock_set, &targets, 1_500),
            vec![1, 3]
        );
    }

    #[test]
    fn filters_allocation_plan() {
        let lock_set = lock_set_with_target(2);
        let plan = vec![(1, 10), (2, 20), (3, 30)];

        assert_eq!(
            filter_allocation_plan(&plan, &lock_set, 1_500),
            vec![(1, 10), (3, 30)]
        );
    }

    #[test]
    fn filters_supply_queue_and_preserves_max_length() {
        let lock_set = lock_set_with_target(2);
        let queue = SupplyQueue {
            entries: VecDeque::from(vec![
                SupplyQueueEntry::new(1, 10),
                SupplyQueueEntry::new(2, 20),
                SupplyQueueEntry::new(3, 30),
            ]),
            max_length: 16,
        };

        let filtered = filter_supply_queue(&queue, &lock_set, 1_500);

        assert_eq!(filtered.max_length, 16);
        assert_eq!(filtered.entries.len(), 2);
        assert_eq!(filtered.entries[0].target_id, 1);
        assert_eq!(filtered.entries[1].target_id, 3);
    }

    #[test]
    fn filters_withdraw_route_and_preserves_target_amount() {
        let lock_set = lock_set_with_target(1);
        let route = WithdrawRoute::from_entries(
            vec![
                WithdrawRouteEntry::new(1, 100),
                WithdrawRouteEntry::new(2, 200),
            ],
            250,
        );

        let filtered = filter_withdraw_route(&route, &lock_set, 1_500);

        assert_eq!(filtered.target_amount, 250);
        assert_eq!(filtered.entries.len(), 1);
        assert_eq!(filtered.entries[0].target_id, 2);
    }
}
