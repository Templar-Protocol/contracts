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

#[test]
fn builds_allocation_plan_with_locks() {
    let lock_set = lock_set_with_target(2);
    let queue = SupplyQueue {
        entries: VecDeque::from(vec![
            SupplyQueueEntry::new(1, 10),
            SupplyQueueEntry::new(2, 20),
            SupplyQueueEntry::new(3, 30),
        ]),
        max_length: 16,
    };

    assert_eq!(
        build_allocation_plan_with_locks(&queue, &lock_set, 1_500),
        vec![(1, 10), (3, 30)]
    );
}

#[test]
fn builds_withdrawal_plan_with_locks() {
    let lock_set = lock_set_with_target(1);
    let route = WithdrawRoute::from_entries(
        vec![
            WithdrawRouteEntry::new(1, 100),
            WithdrawRouteEntry::new(2, 200),
            WithdrawRouteEntry::new(3, 300),
        ],
        450,
    );

    assert_eq!(
        build_withdrawal_plan_with_locks(&route, &lock_set, 1_500),
        vec![(2, 200), (3, 300)]
    );
}

#[test]
fn builds_refresh_plan_with_locks() {
    let lock_set = lock_set_with_target(2);
    let targets = vec![1, 2, 3, 4];

    assert_eq!(
        build_refresh_plan_with_locks(&targets, &lock_set, 1_500),
        vec![1, 3, 4]
    );
}
