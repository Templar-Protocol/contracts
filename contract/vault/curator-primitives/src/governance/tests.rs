use alloc::collections::{BTreeSet, VecDeque};

use super::{
    cap_change_decision, cap_group_cap_change_decision, determine_relaxed, queue_take_mature,
    PendingQueueError, PendingValue, Restrictions, TimelockDecision,
};

#[test]
fn pending_value_maturity_is_time_based() {
    let pending = PendingValue::new("ok", 1_000);

    assert!(!pending.is_mature(999));
    assert!(pending.is_mature(1_000));
    assert!(pending.is_mature(1_001));
}

#[test]
fn queue_take_mature_enforces_timelock() {
    let mut queue = VecDeque::from([PendingValue::new("change", 1_000)]);

    let not_ready = queue_take_mature(&mut queue, 999, |value| *value == "change");
    assert_eq!(not_ready, Err(PendingQueueError::NotMature));
    assert_eq!(queue.len(), 1);

    let ready = queue_take_mature(&mut queue, 1_000, |value| *value == "change");
    assert_eq!(ready, Ok(Some("change")));
    assert!(queue.is_empty());
}

#[test]
fn cap_change_decision_market_new_cap_is_timelocked() {
    let decision = cap_change_decision(None, 100);
    assert_eq!(decision, Ok(TimelockDecision::Timelocked));
}

#[test]
fn cap_group_cap_change_decision_unlimited_to_finite_is_immediate() {
    let from_none = cap_group_cap_change_decision(None, 100);
    assert_eq!(from_none, Ok(TimelockDecision::Immediate));

    let from_zero = cap_group_cap_change_decision(Some(0), 100);
    assert_eq!(from_zero, Ok(TimelockDecision::Immediate));
}

#[test]
fn cap_group_cap_change_decision_finite_to_unlimited_is_timelocked() {
    let decision = cap_group_cap_change_decision(Some(100), 0);
    assert_eq!(decision, Ok(TimelockDecision::Timelocked));
}

#[test]
fn determine_relaxed_paused_to_empty_whitelist_is_not_relaxing() {
    let current = Some(Restrictions::<&str>::Paused);
    let next = Some(Restrictions::Whitelist(BTreeSet::new()));

    assert!(!determine_relaxed(&current, &next));
}

#[test]
fn determine_relaxed_paused_to_nonempty_whitelist_is_relaxing() {
    let current = Some(Restrictions::<&str>::Paused);
    let next = Some(Restrictions::Whitelist(BTreeSet::from(["alice"])));

    assert!(determine_relaxed(&current, &next));
}
