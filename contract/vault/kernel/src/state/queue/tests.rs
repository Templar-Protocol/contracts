extern crate alloc;

use super::*;
use alloc::collections::BTreeMap;

#[test]
#[should_panic(expected = "cached_total_escrow underflow")]
fn dequeue_panics_on_cached_escrow_underflow() {
    let mut pending = BTreeMap::new();
    pending.insert(
        0,
        PendingWithdrawal::new([1u8; 32], [2u8; 32], 100, 200, 0),
    );
    let mut queue = WithdrawQueue::with_state(pending, 0, 1);
    queue.cached_total_escrow = 0;
    queue.dequeue();
}

#[test]
#[should_panic(expected = "cached_total_expected underflow")]
fn dequeue_panics_on_cached_expected_underflow() {
    let mut pending = BTreeMap::new();
    pending.insert(
        0,
        PendingWithdrawal::new([1u8; 32], [2u8; 32], 100, 200, 0),
    );
    let mut queue = WithdrawQueue::with_state(pending, 0, 1);
    queue.cached_total_expected = 0;
    queue.dequeue();
}
