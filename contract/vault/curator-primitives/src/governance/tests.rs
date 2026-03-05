use alloc::collections::VecDeque;

use super::{queue_take_mature, PendingQueueError, PendingValue};

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
