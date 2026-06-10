use super::*;
use crate::test_utils::{owner_addr, receiver_addr};
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

fn make_withdrawal(owner: u8, shares: u128, expected: u128) -> PendingWithdrawal {
    PendingWithdrawal::new(
        owner_addr(owner as u64),
        owner_addr(owner as u64),
        shares,
        expected,
        TimestampNs(1_000_000_000_000), // 1 second in ns
    )
}

/// Shorthand to enqueue a test withdrawal: owner == receiver, ts derived from index.
fn enqueue_simple(queue: &mut WithdrawQueue, index: u64, shares: u128, expected: u128) {
    queue
        .enqueue(
            owner_addr(index),
            owner_addr(index),
            shares,
            expected,
            TimestampNs(index.saturating_mul(1_000_000_000_000)),
            100, // max_pending
        )
        .unwrap();
}

#[test]
fn test_is_valid_withdrawal_amount() {
    assert!(!is_valid_withdrawal_amount(0));
    assert!(!is_valid_withdrawal_amount(999));
    assert!(is_valid_withdrawal_amount(1_000));
    assert!(is_valid_withdrawal_amount(1_000_000));
}

#[test]
fn test_can_enqueue() {
    assert!(can_enqueue(0));
    assert!(can_enqueue(MAX_QUEUE_LENGTH - 1));
    assert!(!can_enqueue(MAX_QUEUE_LENGTH));
    assert!(!can_enqueue(MAX_QUEUE_LENGTH + 1));
}

#[test]
fn test_is_past_cooldown() {
    let requested = 1_000_000_000_000u64; // 1 second
    let cooldown = 60_000_000_000u64; // 60 seconds

    // Not yet past cooldown
    assert!(!is_past_cooldown(
        TimestampNs(requested),
        TimestampNs(requested),
        cooldown
    ));
    assert!(!is_past_cooldown(
        TimestampNs(requested),
        TimestampNs(requested + cooldown - 1),
        cooldown
    ));

    // Past cooldown
    assert!(is_past_cooldown(
        TimestampNs(requested),
        TimestampNs(requested + cooldown),
        cooldown
    ));
    assert!(is_past_cooldown(
        TimestampNs(requested),
        TimestampNs(requested + cooldown + 1),
        cooldown
    ));
}

#[test]
fn test_can_satisfy_withdrawal() {
    let w = make_withdrawal(1, 100, 1000);

    assert!(can_satisfy_withdrawal(&w, 1000));
    assert!(can_satisfy_withdrawal(&w, 2000));
    assert!(!can_satisfy_withdrawal(&w, 999));
    assert!(!can_satisfy_withdrawal(&w, 0));
}

#[test]
fn test_can_partially_satisfy() {
    let w = make_withdrawal(1, 100, 10_000);

    // Can partially satisfy with >= MIN_WITHDRAWAL_ASSETS but < expected
    assert!(can_partially_satisfy(&w, 5_000));
    assert!(can_partially_satisfy(&w, MIN_WITHDRAWAL_ASSETS));

    // Cannot partially satisfy if too small
    assert!(!can_partially_satisfy(&w, MIN_WITHDRAWAL_ASSETS - 1));
    assert!(!can_partially_satisfy(&w, 0));

    // Cannot partially satisfy if meets or exceeds expected
    assert!(!can_partially_satisfy(&w, 10_000));
    assert!(!can_partially_satisfy(&w, 20_000));
}

#[test]
fn test_count_satisfiable() {
    let withdrawals: Vec<PendingWithdrawal> = vec![
        make_withdrawal(1, 100, 1000),
        make_withdrawal(2, 200, 2000),
        make_withdrawal(3, 300, 3000),
    ];

    // Can satisfy all
    let (count, total) = count_satisfiable(&withdrawals, 10000);
    assert_eq!(count, 3);
    assert_eq!(total, 6000);

    // Can satisfy first two
    let (count, total) = count_satisfiable(&withdrawals, 3000);
    assert_eq!(count, 2);
    assert_eq!(total, 3000);

    // Can satisfy only first
    let (count, total) = count_satisfiable(&withdrawals, 1500);
    assert_eq!(count, 1);
    assert_eq!(total, 1000);

    // Cannot satisfy any
    let (count, total) = count_satisfiable(&withdrawals, 500);
    assert_eq!(count, 0);
    assert_eq!(total, 0);
}

#[test]
fn test_compute_settlement_full_redemption() {
    let settlement = compute_settlement(100, 1000, 1000);
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);

    // Also full if actual > expected
    let settlement = compute_settlement(100, 1000, 2000);
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);
}

#[test]
fn test_compute_settlement_cancellation() {
    let settlement = compute_settlement(100, 1000, 0);
    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 100);
}

#[test]
fn test_compute_settlement_partial() {
    // 50% redemption
    let settlement = compute_settlement(100, 1000, 500);
    assert_eq!(settlement.to_burn, 50);
    assert_eq!(settlement.refund, 50);

    // 75% redemption
    let settlement = compute_settlement(100, 1000, 750);
    assert_eq!(settlement.to_burn, 75);
    assert_eq!(settlement.refund, 25);

    // 10% redemption
    let settlement = compute_settlement(100, 1000, 100);
    assert_eq!(settlement.to_burn, 10);
    assert_eq!(settlement.refund, 90);
}

#[test]
fn test_compute_settlement_edge_cases() {
    // Zero escrow shares
    let settlement = compute_settlement(0, 1000, 500);
    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 0);

    let settlement = compute_settlement(100, 0, 500);
    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 100);
}

#[test]
#[should_panic(expected = "duplicate pending withdrawal id")]
fn pending_withdrawals_from_iter_rejects_duplicate_ids() {
    let withdrawal = make_withdrawal(1, 100, 1000);
    let _pending: PendingWithdrawals = vec![(7, withdrawal.clone()), (7, withdrawal)]
        .into_iter()
        .collect();
}

#[test]
#[should_panic(expected = "pending withdrawal entries must be sorted by unique id")]
fn pending_withdrawals_from_sorted_entries_rejects_duplicate_ids() {
    let withdrawal = make_withdrawal(1, 100, 1000);
    let _pending =
        PendingWithdrawals::from_sorted_entries(vec![(7, withdrawal.clone()), (7, withdrawal)]);
}

#[test]
#[should_panic(expected = "pending withdrawal entries must be sorted by unique id")]
fn pending_withdrawals_from_sorted_entries_rejects_unsorted_ids() {
    let withdrawal = make_withdrawal(1, 100, 1000);
    let _pending =
        PendingWithdrawals::from_sorted_entries(vec![(8, withdrawal.clone()), (7, withdrawal)]);
}

#[test]
fn test_compute_full_withdrawal() {
    let w = make_withdrawal(1, 100, 1000);

    // Sufficient assets
    let result = compute_full_withdrawal(&w, 1000);
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.assets_out, 1000);
    assert_eq!(result.settlement.to_burn, 100);
    assert_eq!(result.settlement.refund, 0);

    // Insufficient assets
    let result = compute_full_withdrawal(&w, 500);
    assert!(result.is_none());
}

#[test]
fn test_compute_partial_withdrawal() {
    let w = make_withdrawal(1, 100, 1000);

    let result = compute_partial_withdrawal(&w, 500);
    assert_eq!(result.assets_out, 500);
    assert_eq!(result.settlement.to_burn, 50);
    assert_eq!(result.settlement.refund, 50);

    // If more assets available than expected, caps at expected
    let result = compute_partial_withdrawal(&w, 2000);
    assert_eq!(result.assets_out, 1000);
    assert_eq!(result.settlement.to_burn, 100);
    assert_eq!(result.settlement.refund, 0);
}

#[test]
fn test_compute_idle_settlement() {
    let w = make_withdrawal(1, 100, 1_000);

    let full = compute_idle_settlement(w.escrow_shares, w.expected_assets, 1_000)
        .expect("full idle settlement");
    assert_eq!(full.assets_out, 1_000);
    assert_eq!(full.settlement.to_burn, 100);
    assert_eq!(full.settlement.refund, 0);

    let partial = compute_idle_settlement(w.escrow_shares, 2_000, MIN_WITHDRAWAL_ASSETS)
        .expect("partial idle settlement");
    assert_eq!(partial.assets_out, MIN_WITHDRAWAL_ASSETS);
    assert_eq!(partial.settlement.to_burn, 50);
    assert_eq!(partial.settlement.refund, 50);

    let too_small = compute_idle_settlement(
        w.escrow_shares,
        10_000,
        MIN_WITHDRAWAL_ASSETS.saturating_sub(1),
    )
    .expect("sub-threshold idle settlement still computes settlement math");
    assert_eq!(
        too_small.assets_out,
        MIN_WITHDRAWAL_ASSETS.saturating_sub(1)
    );
    assert_eq!(too_small.settlement.to_burn, 10);
    assert_eq!(too_small.settlement.refund, 90);

    let zero_expected =
        compute_idle_settlement(w.escrow_shares, 0, 5_000).expect("zero expected settlement");
    assert_eq!(zero_expected.assets_out, 0);
    assert_eq!(zero_expected.settlement.to_burn, 100);
    assert_eq!(zero_expected.settlement.refund, 0);
}

#[test]
fn test_compute_queue_status() {
    let withdrawals: Vec<PendingWithdrawal> = vec![
        make_withdrawal(1, 100, 1000),
        make_withdrawal(2, 200, 2000),
        make_withdrawal(3, 300, 3000),
    ];

    let status = compute_queue_status(&withdrawals);
    assert_eq!(status.length, 3);
    assert_eq!(status.total_expected_assets, 6000);
    assert_eq!(status.total_escrow_shares, 600);
}

#[test]
fn test_find_request_status() {
    let withdrawals: Vec<PendingWithdrawal> = vec![
        make_withdrawal(1, 100, 1000),
        make_withdrawal(2, 200, 2000),
        make_withdrawal(3, 300, 3000),
    ];

    // Find alice (first)
    let status = find_request_status(&withdrawals, &owner_addr(1));
    assert!(status.is_some());
    let status = status.unwrap();
    assert_eq!(status.index, 0);
    assert_eq!(status.depth_assets, 0);
    assert_eq!(status.withdrawal.escrow_shares, 100);

    // Find bob (second)
    let status = find_request_status(&withdrawals, &owner_addr(2));
    assert!(status.is_some());
    let status = status.unwrap();
    assert_eq!(status.index, 1);
    assert_eq!(status.depth_assets, 1000);

    // Find charlie (third)
    let status = find_request_status(&withdrawals, &owner_addr(3));
    assert!(status.is_some());
    let status = status.unwrap();
    assert_eq!(status.index, 2);
    assert_eq!(status.depth_assets, 3000);

    // Not found
    let status = find_request_status(&withdrawals, &owner_addr(9));
    assert!(status.is_none());
}

#[test]
fn test_pending_withdrawal_is_past_cooldown() {
    let w = PendingWithdrawal::new(
        owner_addr(1),
        owner_addr(1),
        100,
        1000,
        TimestampNs(1_000_000_000_000), // 1 second
    );

    let cooldown = 60_000_000_000u64; // 60 seconds

    // Not past cooldown
    assert!(!is_past_cooldown(
        w.requested_at_ns,
        TimestampNs(1_000_000_000_000),
        cooldown
    ));
    assert!(!is_past_cooldown(
        w.requested_at_ns,
        TimestampNs(1_059_999_999_999),
        cooldown
    ));

    // Past cooldown
    assert!(is_past_cooldown(
        w.requested_at_ns,
        TimestampNs(1_060_000_000_000),
        cooldown
    ));
    assert!(is_past_cooldown(
        w.requested_at_ns,
        TimestampNs(2_000_000_000_000),
        cooldown
    ));
}

#[test]
fn test_compute_settlement_by_price() {
    // Same price = full burn
    let settlement = compute_settlement_by_price(
        100,
        Wad::from(Wad::SCALE), // 1.0
        Wad::from(Wad::SCALE), // 1.0
    );
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);

    // Higher price = full burn
    let settlement = compute_settlement_by_price(
        100,
        Wad::from(Wad::SCALE * 2), // 2.0
        Wad::from(Wad::SCALE),     // 1.0
    );
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);

    // Half price = half burn
    let settlement = compute_settlement_by_price(
        100,
        Wad::from(Wad::SCALE / 2), // 0.5
        Wad::from(Wad::SCALE),     // 1.0
    );
    assert_eq!(settlement.to_burn, 50);
    assert_eq!(settlement.refund, 50);

    // One-third price: ceil(100 * 1/3) = 34 burn, 66 refund
    // Ceil rounding ensures vault keeps more shares (rounds against user).
    let settlement = compute_settlement_by_price(
        100,
        Wad::from(Wad::SCALE / 3), // 0.333...
        Wad::from(Wad::SCALE),     // 1.0
    );
    assert_eq!(settlement.to_burn, 34);
    assert_eq!(settlement.refund, 66);
}

#[test]
fn test_withdraw_queue_new() {
    let queue = WithdrawQueue::new();
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.next_withdraw_to_execute, 0);
    assert_eq!(queue.next_pending_withdrawal_id, 0);
    assert!(queue.check_invariants());
}

#[test]
fn test_withdraw_queue_enqueue() {
    let mut queue = WithdrawQueue::new();
    let max_pending = 100u32;

    let id = queue
        .enqueue(
            owner_addr(1),
            owner_addr(1),
            100,
            1000,
            TimestampNs(1_000_000_000_000),
            max_pending,
        )
        .unwrap();

    assert_eq!(id, 0);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.next_pending_withdrawal_id, 1);
    assert_eq!(queue.next_withdraw_to_execute, 0);
    assert!(queue.check_invariants());
}

#[test]
fn test_withdraw_queue_enqueue_multiple() {
    let mut queue = WithdrawQueue::new();
    let max_pending = 100u32;

    let id1 = queue
        .enqueue(
            owner_addr(1),
            owner_addr(1),
            100,
            1000,
            TimestampNs(1_000_000_000_000),
            max_pending,
        )
        .unwrap();
    let id2 = queue
        .enqueue(
            owner_addr(2),
            owner_addr(2),
            200,
            2000,
            TimestampNs(2_000_000_000_000),
            max_pending,
        )
        .unwrap();
    let id3 = queue
        .enqueue(
            owner_addr(3),
            owner_addr(3),
            300,
            3000,
            TimestampNs(3_000_000_000_000),
            max_pending,
        )
        .unwrap();

    assert_eq!(id1, 0);
    assert_eq!(id2, 1);
    assert_eq!(id3, 2);
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.next_pending_withdrawal_id, 3);
    assert_eq!(queue.next_withdraw_to_execute, 0);
    assert!(queue.check_invariants());
}

#[test]
fn test_withdraw_queue_enqueue_full() {
    let mut queue = WithdrawQueue::new();
    let max_pending = 2u32;

    // Enqueue up to max
    queue
        .enqueue(
            owner_addr(1),
            owner_addr(1),
            100,
            1000,
            TimestampNs(1_000_000_000_000),
            max_pending,
        )
        .unwrap();
    queue
        .enqueue(
            owner_addr(2),
            owner_addr(2),
            200,
            2000,
            TimestampNs(2_000_000_000_000),
            max_pending,
        )
        .unwrap();

    // Should fail when full
    let result = queue.enqueue(
        owner_addr(3),
        owner_addr(3),
        300,
        3000,
        TimestampNs(3_000_000_000_000),
        max_pending,
    );
    assert!(result.is_err());
    match result {
        Err(QueueError::QueueFull { current, max }) => {
            assert_eq!(current, 2);
            assert_eq!(max, 2);
        }
        _ => panic!("Expected QueueFull error"),
    }
}

#[test]
fn test_withdraw_queue_enqueue_id_overflow_fails() {
    let mut queue = WithdrawQueue::new();
    queue.next_pending_withdrawal_id = u64::MAX;

    let result = queue.enqueue(owner_addr(1), owner_addr(1), 100, 1000, TimestampNs(0), 10);

    assert!(matches!(
        result,
        Err(QueueError::InvariantViolation { message }) if message == "next_pending_withdrawal_id overflow"
    ));
    assert!(queue.is_empty());
}

#[test]
fn test_withdraw_queue_head_non_destructive() {
    let mut queue = WithdrawQueue::new();

    // Empty queue
    assert!(queue.head().is_none());

    // Add items
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    let (id, withdrawal) = queue.head().unwrap();
    assert_eq!(id, 0);
    assert_eq!(withdrawal.owner, owner_addr(1));
    assert_eq!(withdrawal.escrow_shares, 100);

    let (id2, _) = queue.head().unwrap();
    assert_eq!(id2, 0);
    assert_eq!(queue.len(), 2); // Length unchanged
}

#[test]
fn test_withdraw_queue_head() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);

    let (id, withdrawal) = queue.head().unwrap();
    assert_eq!(id, 0);
    assert_eq!(withdrawal.owner, owner_addr(1));
}

#[test]
fn test_withdraw_queue_dequeue() {
    let mut queue = WithdrawQueue::new();

    // Empty queue
    assert!(queue.dequeue().is_none());

    // Add items
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);
    enqueue_simple(&mut queue, 3, 300, 3000);

    // Dequeue first
    let (id1, w1) = queue.dequeue().unwrap();
    assert_eq!(id1, 0);
    assert_eq!(w1.owner, owner_addr(1));
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.next_withdraw_to_execute, 1);
    assert!(queue.check_invariants());

    // Dequeue second
    let (id2, w2) = queue.dequeue().unwrap();
    assert_eq!(id2, 1);
    assert_eq!(w2.owner, owner_addr(2));
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.next_withdraw_to_execute, 2);
    assert!(queue.check_invariants());

    // Dequeue third
    let (id3, w3) = queue.dequeue().unwrap();
    assert_eq!(id3, 2);
    assert_eq!(w3.owner, owner_addr(3));
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.next_withdraw_to_execute, 3);
    assert!(queue.check_invariants());

    // Empty again
    assert!(queue.dequeue().is_none());
}

#[test]
fn test_withdraw_queue_get() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    // Get existing
    let w = queue.get(0).unwrap();
    assert_eq!(w.owner, owner_addr(1));

    let w = queue.get(1).unwrap();
    assert_eq!(w.owner, owner_addr(2));

    // Get non-existing
    assert!(queue.get(2).is_none());
    assert!(queue.get(999).is_none());
}

#[test]
fn test_withdraw_queue_contains() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);

    assert!(queue.contains(0));
    assert!(!queue.contains(1));
    assert!(!queue.contains(999));
}

#[test]
fn test_withdraw_queue_iter() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    let items: Vec<_> = queue.iter().collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].0, 0);
    assert_eq!(items[0].1.owner, owner_addr(1));
    assert_eq!(items[1].0, 1);
    assert_eq!(items[1].1.owner, owner_addr(2));
}

#[test]
fn test_withdraw_queue_status() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);
    enqueue_simple(&mut queue, 3, 300, 3000);

    let status = queue.status();
    assert_eq!(status.length, 3);
    assert_eq!(status.total_expected_assets, 6000);
    assert_eq!(status.total_escrow_shares, 600);
}

#[test]
fn test_withdraw_queue_total_escrow_shares() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    assert_eq!(queue.total_escrow_shares(), 300);
}

#[test]
fn test_withdraw_queue_total_expected_assets() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    assert_eq!(queue.total_expected_assets(), 3000);
}

#[test]
fn test_withdraw_queue_check_invariants() {
    let mut queue = WithdrawQueue::new();
    assert!(queue.check_invariants());

    // After enqueue
    enqueue_simple(&mut queue, 1, 100, 1000);
    assert!(queue.check_invariants());

    // After dequeue
    queue.dequeue();
    assert!(queue.check_invariants());
}

#[test]
fn test_withdraw_queue_check_invariants_with_max() {
    let mut queue = WithdrawQueue::new();
    enqueue_simple(&mut queue, 1, 100, 1000);
    enqueue_simple(&mut queue, 2, 200, 2000);

    // Valid max
    assert!(queue.check_invariants_with_max(100));
    assert!(queue.check_invariants_with_max(1024));

    // Max too low
    assert!(!queue.check_invariants_with_max(1));

    // Max exceeds MAX_PENDING
    assert!(!queue.check_invariants_with_max(2000));
}

#[test]
fn test_withdraw_queue_invariant_violation_head_missing() {
    // Manually create an invalid queue state
    let mut pending = BTreeMap::new();
    pending.insert(
        5,
        PendingWithdrawal::new(
            owner_addr(1),
            owner_addr(1),
            100,
            1000,
            TimestampNs(1_000_000_000_000),
        ),
    );

    let queue = WithdrawQueue::with_state(
        pending, 0, // head points to non-existent ID 0
        6,
    );

    assert!(!queue.check_invariants());
}

#[test]
fn test_withdraw_queue_invariant_violation_head_exceeds_next() {
    let queue = WithdrawQueue::with_state(
        BTreeMap::new(),
        10, // head > next_pending_withdrawal_id
        5,
    );

    assert!(!queue.check_invariants());
}

#[test]
fn test_withdraw_queue_fifo_ordering() {
    let mut queue = WithdrawQueue::new();

    // Enqueue in order
    for i in 0..5u64 {
        enqueue_simple(&mut queue, i, (i as u128 + 1) * 100, (i as u128 + 1) * 1000);
    }

    // Dequeue should maintain FIFO order
    for i in 0..5 {
        let (id, w) = queue.dequeue().unwrap();
        assert_eq!(id, i);
        assert_eq!(w.owner, owner_addr(i));
    }
}

#[test]
fn test_withdraw_queue_can_enqueue_respects_max_pending() {
    let queue = WithdrawQueue::new();

    assert!(queue.can_enqueue(1));
    assert!(queue.can_enqueue(100));
    assert!(queue.can_enqueue(1024));
    assert!(queue.can_enqueue(2000)); // clamped to MAX_PENDING
}

#[test]
fn test_withdraw_queue_enqueue_withdrawal() {
    let mut queue = WithdrawQueue::new();
    let max_pending = 100u32;

    let w = PendingWithdrawal::new(
        owner_addr(1),
        owner_addr(2),
        100,
        1000,
        TimestampNs(1_000_000_000_000),
    );

    let id = queue.enqueue_withdrawal(w.clone(), max_pending).unwrap();
    assert_eq!(id, 0);

    let stored = queue.get(0).unwrap();
    assert_eq!(stored.owner, owner_addr(1));
    assert_eq!(stored.receiver, owner_addr(2));
}

#[test]
fn test_withdraw_queue_empty_operations() {
    let queue = WithdrawQueue::new();

    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert!(queue.head().is_none());
    assert!(queue.get(0).is_none());
    assert!(!queue.contains(0));
    assert_eq!(queue.total_escrow_shares(), 0);
    assert_eq!(queue.total_expected_assets(), 0);

    let status = queue.status();
    assert_eq!(status.length, 0);
    assert_eq!(status.total_escrow_shares, 0);
    assert_eq!(status.total_expected_assets, 0);
}

use proptest::prelude::*;

fn arb_withdrawal() -> impl Strategy<Value = PendingWithdrawal> {
    (
        1u32..1000u32,
        1u128..=u64::MAX as u128,
        MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
        0u64..u64::MAX,
    )
        .prop_map(|(owner_idx, shares, expected, ts)| {
            PendingWithdrawal::new(
                owner_addr(owner_idx as u64),
                owner_addr(owner_idx as u64),
                shares,
                expected,
                TimestampNs(ts),
            )
        })
}

fn arb_queue(max_len: usize) -> impl Strategy<Value = Vec<PendingWithdrawal>> {
    proptest::collection::vec(arb_withdrawal(), 0..=max_len)
}

proptest! {
    #[test]
    fn count_satisfiable_monotonic_in_assets(
        queue in arb_queue(10),
        assets1 in 0u128..=u64::MAX as u128,
        assets2 in 0u128..=u64::MAX as u128,
    ) {
        let (lo, hi) = if assets1 <= assets2 { (assets1, assets2) } else { (assets2, assets1) };
        let (count_lo, total_lo) = count_satisfiable(&queue, lo);
        let (count_hi, total_hi) = count_satisfiable(&queue, hi);

        prop_assert!(count_lo <= count_hi, "count not monotonic: {} > {}", count_lo, count_hi);
        prop_assert!(total_lo <= total_hi, "total not monotonic: {} > {}", total_lo, total_hi);
    }

    #[test]
    fn count_satisfiable_total_bounded(
        queue in arb_queue(10),
        available in 0u128..=u64::MAX as u128,
    ) {
        let (_, total) = count_satisfiable(&queue, available);
        prop_assert!(total <= available, "total {} > available {}", total, available);
    }

    #[test]
    fn count_satisfiable_respects_fifo(
        queue in arb_queue(10),
        available in 0u128..=u64::MAX as u128,
    ) {
        let (count, total) = count_satisfiable(&queue, available);

        let sum: u128 = queue.iter().take(count as usize).map(|w| w.expected_assets).sum();
        prop_assert_eq!(sum, total, "sum mismatch: {} vs {}", sum, total);

        if (count as usize) < queue.len() {
            let next = &queue[count as usize];
            prop_assert!(
                total.saturating_add(next.expected_assets) > available,
                "next item should not fit"
            );
        }
    }

    #[test]
    fn compute_settlement_conserves_shares(
        escrow_shares in 0u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
        actual_assets in 0u128..=u64::MAX as u128,
    ) {
        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);
        let total = settlement.to_burn.saturating_add(settlement.refund);
        prop_assert_eq!(total, escrow_shares, "shares not conserved: {} != {}", total, escrow_shares);
    }

    #[test]
    fn compute_settlement_full_burn_on_full_redemption(
        escrow_shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
        extra in 0u128..=1_000_000u128,
    ) {
        let actual_assets = expected_assets.saturating_add(extra);
        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

        prop_assert_eq!(settlement.to_burn, escrow_shares, "should burn all");
        prop_assert_eq!(settlement.refund, 0, "should refund none");
    }

    #[test]
    fn compute_settlement_full_refund_on_cancellation(
        escrow_shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
    ) {
        let settlement = compute_settlement(escrow_shares, expected_assets, 0);

        prop_assert_eq!(settlement.to_burn, 0, "should burn none");
        prop_assert_eq!(settlement.refund, escrow_shares, "should refund all");
    }

    #[test]
    fn compute_settlement_proportional(
        escrow_shares in 1u128..=1_000_000_000u128,
        expected_assets in 1u128..=1_000_000_000u128,
        actual_ratio_pct in 1u8..100u8,
    ) {
        let actual_assets = (expected_assets * actual_ratio_pct as u128) / 100;
        if actual_assets == 0 || actual_assets >= expected_assets {
            return Ok(());
        }

        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

        let expected_burn = (escrow_shares * actual_assets) / expected_assets;
        let diff = settlement.to_burn.abs_diff(expected_burn);

        prop_assert!(diff <= 1, "burn not proportional: expected ~{}, got {}", expected_burn, settlement.to_burn);
    }

    #[test]
    fn compute_settlement_by_price_conserves_shares(
        escrow_shares in 0u128..=u64::MAX as u128,
        current_price in 1u128..=Wad::SCALE * 10,
        original_price in 1u128..=Wad::SCALE * 10,
    ) {
        let settlement = compute_settlement_by_price(
            escrow_shares,
            Wad::from(current_price),
            Wad::from(original_price),
        );
        let total = settlement.to_burn.saturating_add(settlement.refund);
        prop_assert_eq!(total, escrow_shares);
    }

    #[test]
    fn compute_queue_status_length_correct(
        queue in arb_queue(20),
    ) {
        let status = compute_queue_status(&queue);
        prop_assert_eq!(status.length as usize, queue.len());
    }

    #[test]
    fn compute_queue_status_totals_correct(
        queue in arb_queue(20),
    ) {
        let status = compute_queue_status(&queue);

        let expected_assets: u128 = queue.iter().map(|w| w.expected_assets).sum();
        let escrow_shares: u128 = queue.iter().map(|w| w.escrow_shares).sum();

        prop_assert_eq!(status.total_expected_assets, expected_assets);
        prop_assert_eq!(status.total_escrow_shares, escrow_shares);
    }

    #[test]
    fn find_request_status_depth_correct(
        queue in arb_queue(10),
    ) {
        if queue.is_empty() {
            return Ok(());
        }

        let owner = &queue[0].owner;
        let status = find_request_status(&queue, owner);

        prop_assert!(status.is_some());
        let status = status.unwrap();

        let expected_depth: u128 = queue.iter().take(status.index as usize).map(|w| w.expected_assets).sum();
        prop_assert_eq!(status.depth_assets, expected_depth);
    }

    #[test]
    fn is_valid_withdrawal_amount_boundary(
        amount in 0u128..=MIN_WITHDRAWAL_ASSETS * 2,
    ) {
        let valid = is_valid_withdrawal_amount(amount);
        prop_assert_eq!(valid, amount >= MIN_WITHDRAWAL_ASSETS);
    }

    #[test]
    fn can_enqueue_boundary(
        length in 0u32..=MAX_QUEUE_LENGTH + 10,
    ) {
        let can = can_enqueue(length);
        prop_assert_eq!(can, length < MAX_QUEUE_LENGTH);
    }

    #[test]
    fn is_past_cooldown_consistency(
        requested_at in 0u64..=u64::MAX / 2,
        cooldown in 0u64..=u64::MAX / 4,
        delta in 0u64..=u64::MAX / 4,
    ) {
        let now = requested_at.saturating_add(delta);
        let threshold = requested_at.saturating_add(cooldown);
        let past = is_past_cooldown(TimestampNs(requested_at), TimestampNs(now), cooldown);
        prop_assert_eq!(past, now >= threshold);
    }

    #[test]
    fn can_satisfy_withdrawal_consistency(
        expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
        available in 0u128..=u64::MAX as u128,
    ) {
        let w = PendingWithdrawal::new(
            owner_addr(1),
            receiver_addr(1),
            1000,
            expected,
            TimestampNs(0),
        );
        let can = can_satisfy_withdrawal(&w, available);
        prop_assert_eq!(can, available >= expected);
    }

    #[test]
    fn can_partially_satisfy_consistency(
        expected in MIN_WITHDRAWAL_ASSETS + 1..=u64::MAX as u128,
        available in 0u128..=u64::MAX as u128,
    ) {
        let w = PendingWithdrawal::new(
            owner_addr(1),
            receiver_addr(1),
            1000,
            expected,
            TimestampNs(0),
        );
        let can = can_partially_satisfy(&w, available);
        let should = available > 0 && available < expected && available >= MIN_WITHDRAWAL_ASSETS;
        prop_assert_eq!(can, should);
    }

    #[test]
    fn compute_full_withdrawal_consistency(
        shares in 1u128..=u64::MAX as u128,
        expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
        available in 0u128..=u64::MAX as u128,
    ) {
        let w = PendingWithdrawal::new(
            owner_addr(1),
            receiver_addr(1),
            shares,
            expected,
            TimestampNs(0),
        );
        let result = compute_full_withdrawal(&w, available);
        let can = can_satisfy_withdrawal(&w, available);

        prop_assert_eq!(result.is_some(), can);
    }

    #[test]
    fn compute_partial_withdrawal_bounded(
        shares in 1u128..=u64::MAX as u128,
        expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
        available in 0u128..=u64::MAX as u128,
    ) {
        let w = PendingWithdrawal::new(
            owner_addr(1),
            receiver_addr(1),
            shares,
            expected,
            TimestampNs(0),
        );
        let result = compute_partial_withdrawal(&w, available);

        prop_assert!(result.assets_out <= expected);
        prop_assert!(result.assets_out <= available);
    }

    #[test]
    fn compute_idle_settlement_consistency(
        shares in 1u128..=u64::MAX as u128,
        expected in 0u128..=u64::MAX as u128,
        available in 0u128..=u64::MAX as u128,
    ) {
        let result = compute_idle_settlement(shares, expected, available);

        if expected == 0 {
            let result = result.expect("zero-expected settlements should always resolve");
            prop_assert_eq!(result.assets_out, 0);
            if available > 0 {
                prop_assert_eq!(result.settlement.to_burn, shares);
                prop_assert_eq!(result.settlement.refund, 0);
            } else {
                prop_assert_eq!(result.settlement.to_burn, 0);
                prop_assert_eq!(result.settlement.refund, shares);
            }
            return Ok(());
        }

        if available == 0 {
            prop_assert!(result.is_none());
            return Ok(());
        }

        let result = result.expect("eligible idle settlements should resolve");
        prop_assert!(result.assets_out <= expected);
        prop_assert!(result.assets_out <= available);
        prop_assert_eq!(
            result
                .settlement
                .to_burn
                .saturating_add(result.settlement.refund),
            shares
        );
    }

    #[test]
    fn withdraw_queue_enqueue_increases_length(
        num_enqueues in 1usize..20usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for i in 0..num_enqueues {
            let len_before = queue.len();
            queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();
            prop_assert_eq!(queue.len(), len_before + 1);
        }
    }

    #[test]
    fn withdraw_queue_dequeue_decreases_length(
        num_enqueues in 1usize..20usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for i in 0..num_enqueues {
            queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();
        }

        for _ in 0..num_enqueues {
            let len_before = queue.len();
            queue.dequeue();
            prop_assert_eq!(queue.len(), len_before - 1);
        }
    }

    #[test]
    fn withdraw_queue_invariants_maintained(
        operations in proptest::collection::vec(0u8..2u8, 0..50),
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;
        let mut counter = 0u64;

        for op in operations {
            if op == 0 && queue.len() < max_pending as usize {
                queue.enqueue(
                    owner_addr(counter),
                    receiver_addr(counter),
                    100,
                    1000,
                    TimestampNs(counter),
                    max_pending,
                ).unwrap();
                counter += 1;
            } else if op == 1 && !queue.is_empty() {
                queue.dequeue();
            }
            prop_assert!(queue.check_invariants(), "Invariant violated after operation");
        }
    }

    #[test]
    fn withdraw_queue_fifo_ordering(
        num_items in 1usize..20usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for i in 0..num_items {
            queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                (i as u128) + 1,
                (i as u128 + 1) * 1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();
        }

        for i in 0..num_items {
            let (id, w) = queue.dequeue().unwrap();
            prop_assert_eq!(id, i as u64, "ID mismatch at position {}", i);
            prop_assert_eq!(w.owner, owner_addr(i as u64), "Owner mismatch at position {}", i);
        }
    }

    #[test]
    fn withdraw_queue_id_monotonic(
        num_enqueues in 1usize..20usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;
        let mut last_id: Option<u64> = None;

        for i in 0..num_enqueues {
            let id = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();

            if let Some(prev) = last_id {
                prop_assert!(id > prev, "ID not monotonically increasing");
            }
            last_id = Some(id);
        }
    }

    #[test]
    fn withdraw_queue_head_bounded(
        operations in proptest::collection::vec(0u8..2u8, 0..50),
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;
        let mut counter = 0u64;

        for op in operations {
            if op == 0 && queue.len() < max_pending as usize {
                queue.enqueue(
                    owner_addr(counter),
                    receiver_addr(counter),
                    100,
                    1000,
                    TimestampNs(counter),
                    max_pending,
                ).unwrap();
                counter += 1;
            } else if op == 1 && !queue.is_empty() {
                queue.dequeue();
            }
            prop_assert!(
                queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id,
                "Head {} > next_id {}",
                queue.next_withdraw_to_execute,
                queue.next_pending_withdrawal_id
            );
        }
    }

    #[test]
    fn withdraw_queue_total_escrow_correct(
        withdrawals in arb_queue(10),
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for w in &withdrawals {
            let _ = queue.enqueue_withdrawal(w.clone(), max_pending);
        }

        let expected: u128 = queue.iter().map(|(_, w)| w.escrow_shares).sum();
        prop_assert_eq!(queue.total_escrow_shares(), expected);
    }

    #[test]
    fn withdraw_queue_total_expected_correct(
        withdrawals in arb_queue(10),
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for w in &withdrawals {
            let _ = queue.enqueue_withdrawal(w.clone(), max_pending);
        }

        let expected: u128 = queue.iter().map(|(_, w)| w.expected_assets).sum();
        prop_assert_eq!(queue.total_expected_assets(), expected);
    }

    #[test]
    fn withdraw_queue_length_bounded(
        max_pending in 1u32..50u32,
        num_attempts in 0usize..100usize,
    ) {
        let mut queue = WithdrawQueue::new();

        for i in 0..num_attempts {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                TimestampNs(i as u64),
                max_pending,
            );
        }

        prop_assert!(
            queue.len() <= max_pending as usize,
            "Queue length {} exceeds max {}",
            queue.len(),
            max_pending
        );
    }

    #[test]
    fn withdraw_queue_head_is_stable(
        num_enqueues in 1usize..10usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for i in 0..num_enqueues {
            queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();
        }

        let first = queue.head();
        let second = queue.head();

        prop_assert_eq!(first, second);
    }

    #[test]
    fn withdraw_queue_get_by_id(
        num_enqueues in 1usize..10usize,
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;
        let mut ids = alloc::vec::Vec::new();

        for i in 0..num_enqueues {
            let id = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                (i as u128) + 1,
                (i as u128 + 1) * 1000,
                TimestampNs(i as u64),
                max_pending,
            ).unwrap();
            ids.push(id);
        }

        for (i, id) in ids.iter().enumerate() {
            let w = queue.get(*id).unwrap();
            prop_assert_eq!(&w.owner, &owner_addr(i as u64));
            prop_assert_eq!(w.escrow_shares, (i as u128) + 1);
        }
    }

    #[test]
    fn withdraw_queue_status_matches(
        withdrawals in arb_queue(10),
    ) {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        for w in &withdrawals {
            let _ = queue.enqueue_withdrawal(w.clone(), max_pending);
        }

        let status = queue.status();
        prop_assert_eq!(status.length as usize, queue.len());
        prop_assert_eq!(status.total_escrow_shares, queue.total_escrow_shares());
        prop_assert_eq!(status.total_expected_assets, queue.total_expected_assets());
    }
}

#[test]
#[should_panic]
fn dequeue_panics_on_cached_escrow_underflow() {
    use alloc::collections::BTreeMap;
    let mut pending = BTreeMap::new();
    pending.insert(
        0,
        PendingWithdrawal::new(
            Address([1u8; 32]),
            Address([2u8; 32]),
            100,
            200,
            TimestampNs(0),
        ),
    );
    let mut queue = WithdrawQueue::with_state(pending, 0, 1);
    queue.cached_total_escrow = 0;
    queue.dequeue();
}

#[test]
#[should_panic]
fn dequeue_panics_on_cached_expected_underflow() {
    use alloc::collections::BTreeMap;
    let mut pending = BTreeMap::new();
    pending.insert(
        0,
        PendingWithdrawal::new(
            Address([1u8; 32]),
            Address([2u8; 32]),
            100,
            200,
            TimestampNs(0),
        ),
    );
    let mut queue = WithdrawQueue::with_state(pending, 0, 1);
    queue.cached_total_expected = 0;
    queue.dequeue();
}
