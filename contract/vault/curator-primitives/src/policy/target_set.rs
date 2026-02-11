//! Shared helpers for target-set validation.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use templar_vault_kernel::TargetId;

use super::{
    market_lock::MarketLockSet,
    refresh_plan::{RefreshPlan, RefreshPlanError},
    withdraw_route::{build_withdraw_route, WithdrawRouteError},
};

/// Returns the first duplicate item found in insertion order.
#[must_use]
pub fn find_first_duplicate<T: Ord + Copy>(items: &[T]) -> Option<T> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(*item) {
            return Some(*item);
        }
    }
    None
}

/// Returns true when all items are unique.
#[must_use]
pub fn has_unique_items<T: Ord + Copy>(items: &[T]) -> bool {
    find_first_duplicate(items).is_none()
}

/// Returns the first duplicate target ID in insertion order.
#[must_use]
pub fn find_duplicate_target_id(targets: &[TargetId]) -> Option<TargetId> {
    find_first_duplicate(targets)
}

/// Returns true when all target IDs are unique.
#[must_use]
pub fn validate_no_duplicate_targets(targets: &[TargetId]) -> bool {
    has_unique_items(targets)
}

/// Build a withdraw plan from target principals.
pub fn build_withdraw_plan_from_target_principals(
    principals: &[(TargetId, u128)],
    target_amount: u128,
) -> Result<Vec<(TargetId, u128)>, WithdrawRouteError> {
    let route = build_withdraw_route(principals, target_amount)?;
    Ok(route
        .entries
        .iter()
        .map(|entry| (entry.target_id, entry.max_amount))
        .collect())
}

/// Return locked target IDs from a candidate target list.
#[must_use]
pub fn find_locked_targets(
    lock_set: &MarketLockSet,
    targets: &[TargetId],
    current_ns: u64,
) -> Vec<TargetId> {
    lock_set.find_locked_targets(targets, current_ns)
}

/// Check if a target is currently locked.
#[must_use]
pub fn is_target_locked(lock_set: &MarketLockSet, target: TargetId, current_ns: u64) -> bool {
    lock_set.is_locked(target, current_ns)
}

/// Return all currently locked target IDs.
#[must_use]
pub fn get_locked_targets(lock_set: &MarketLockSet, current_ns: u64) -> Vec<TargetId> {
    lock_set.locked_targets(current_ns)
}

/// Build and validate a refresh plan from target IDs.
pub fn build_refresh_plan_from_targets(
    targets: &[TargetId],
    cooldown_ns: u64,
    last_refresh_ns: u64,
) -> Result<RefreshPlan, RefreshPlanError> {
    let plan = RefreshPlan::new(targets.to_vec())
        .with_cooldown(cooldown_ns)
        .with_last_refresh(last_refresh_ns);
    plan.validate()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::policy::market_lock::MarketLock;

    #[test]
    fn finds_first_duplicate() {
        assert_eq!(find_first_duplicate(&[1u32, 2, 3]), None);
        assert_eq!(find_first_duplicate(&[1u32, 2, 1]), Some(1));
        assert_eq!(find_first_duplicate(&[1u32, 2, 2, 3]), Some(2));
    }

    #[test]
    fn validates_uniqueness() {
        assert!(has_unique_items(&[1u32, 2, 3]));
        assert!(!has_unique_items(&[1u32, 2, 1]));
    }

    #[test]
    fn validates_no_duplicate_targets() {
        assert!(validate_no_duplicate_targets(&[1, 2, 3]));
        assert!(!validate_no_duplicate_targets(&[1, 2, 1]));
        assert_eq!(find_duplicate_target_id(&[1, 2, 1]), Some(1));
    }

    #[test]
    fn builds_withdraw_plan_from_target_principals() {
        let principals = vec![(1, 100), (2, 200), (3, 300)];
        let plan = build_withdraw_plan_from_target_principals(&principals, 250).unwrap();

        assert_eq!(plan, vec![(3, 300), (2, 200), (1, 100)]);
    }

    #[test]
    fn target_lock_helpers_delegate_to_lock_set() {
        let mut set = MarketLockSet::new();
        set = set.acquire(MarketLock::new(2, 1_000), 1_000).unwrap();

        let targets = vec![1, 2, 3];
        assert_eq!(find_locked_targets(&set, &targets, 1_500), vec![2]);
        assert!(is_target_locked(&set, 2, 1_500));
        assert!(!is_target_locked(&set, 1, 1_500));
        assert_eq!(get_locked_targets(&set, 1_500), vec![2]);
    }

    #[test]
    fn builds_refresh_plan_from_targets() {
        let plan = build_refresh_plan_from_targets(&[1, 2, 3], 100, 50).unwrap();
        assert_eq!(plan.targets, vec![1, 2, 3]);
        assert_eq!(plan.cooldown_ns(), 100);
        assert_eq!(plan.last_refresh_ns(), Some(50));
    }
}
