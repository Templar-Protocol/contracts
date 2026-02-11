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
