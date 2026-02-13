//! Shared helpers for filtering locked targets from plans.

use alloc::vec::Vec;

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
    let entries: Vec<SupplyQueueEntry> = queue
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

/// Build an allocation plan from a queue while excluding locked targets.
#[must_use]
pub fn build_allocation_plan_with_locks(
    queue: &SupplyQueue,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    filter_supply_queue(queue, lock_set, current_ns).to_allocation_plan()
}

/// Build a withdraw plan from a route while excluding locked targets.
#[must_use]
pub fn build_withdrawal_plan_with_locks(
    route: &WithdrawRoute,
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<(TargetId, u128)> {
    filter_withdraw_route(route, lock_set, current_ns).to_withdrawal_plan()
}

/// Build a refresh target list while excluding locked targets.
#[must_use]
pub fn build_refresh_plan_with_locks(
    targets: &[TargetId],
    lock_set: &MarketLockSet,
    current_ns: u64,
) -> Vec<TargetId> {
    filter_unlocked_targets(lock_set, targets, current_ns)
}

#[cfg(test)]
mod tests;
