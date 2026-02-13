use super::*;
use crate::test_utils::owner_addr;
use alloc::vec;
use alloc::vec::Vec;

fn make_entry(owner: u64, shares: u128, expected: u128) -> EscrowEntry {
    EscrowEntry::new(
        owner_addr(owner),
        shares,
        1_000_000_000_000, // 1 second in ns
        expected,
    )
}

#[test]
fn test_escrow_entry_is_empty() {
    let entry = make_entry(1, 0, 1000);
    assert!(entry.is_empty());

    let entry = make_entry(1, 100, 1000);
    assert!(!entry.is_empty());
}

#[test]
fn test_apply_settlement_valid() {
    let entry = make_entry(1, 100, 1000);
    let settlement = EscrowSettlement::partial(60, 40);

    let result = apply_settlement(&entry, &settlement);
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.burned, 60);
    assert_eq!(result.refunded, 40);
    assert_eq!(result.remaining, 0);
}

#[test]
fn test_apply_settlement_partial() {
    let entry = make_entry(1, 100, 1000);
    let settlement = EscrowSettlement::partial(30, 20);

    let result = apply_settlement(&entry, &settlement);
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.burned, 30);
    assert_eq!(result.refunded, 20);
    assert_eq!(result.remaining, 50);
}

#[test]
fn test_apply_settlement_exceeds_escrow() {
    let entry = make_entry(1, 100, 1000);
    let settlement = EscrowSettlement::partial(80, 30); // 110 > 100

    let result = apply_settlement(&entry, &settlement);
    assert!(result.is_none());
}

#[test]
fn test_settle_full_burn() {
    let entry = make_entry(1, 100, 1000);
    let settlement = EscrowSettlement::burn_all(entry.shares);

    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);
}

#[test]
fn test_settle_full_refund() {
    let entry = make_entry(1, 100, 1000);
    let settlement = EscrowSettlement::refund_all(entry.shares);

    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 100);
}

#[test]
fn test_settle_proportional_full() {
    let entry = make_entry(1, 100, 1000);

    // Full assets available
    let settlement = settle_proportional(&entry, 1000);
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);

    // More than expected
    let settlement = settle_proportional(&entry, 2000);
    assert_eq!(settlement.to_burn, 100);
    assert_eq!(settlement.refund, 0);
}

#[test]
fn test_settle_proportional_zero() {
    let entry = make_entry(1, 100, 1000);

    let settlement = settle_proportional(&entry, 0);
    assert_eq!(settlement.to_burn, 0);
    assert_eq!(settlement.refund, 100);
}

#[test]
fn test_settle_proportional_partial() {
    let entry = make_entry(1, 100, 1000);

    // 50% available
    let settlement = settle_proportional(&entry, 500);
    assert_eq!(settlement.to_burn, 50);
    assert_eq!(settlement.refund, 50);

    // 75% available
    let settlement = settle_proportional(&entry, 750);
    assert_eq!(settlement.to_burn, 75);
    assert_eq!(settlement.refund, 25);
}

#[test]
fn test_can_apply_settlement() {
    let entry = make_entry(1, 100, 1000);

    // Valid settlement
    assert!(can_apply_settlement(
        &entry,
        &EscrowSettlement::partial(50, 50)
    ));
    assert!(can_apply_settlement(
        &entry,
        &EscrowSettlement::burn_all(100)
    ));
    assert!(can_apply_settlement(
        &entry,
        &EscrowSettlement::refund_all(100)
    ));

    // Invalid settlement (exceeds shares)
    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::partial(60, 50)
    ));
    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::burn_all(101)
    ));
}

#[test]
fn test_is_stale() {
    let entry = make_entry(1, 100, 1000);
    let max_age = 60_000_000_000u64; // 60 seconds

    // Not stale
    assert!(!is_stale(&entry, 1_000_000_000_000, max_age));
    assert!(!is_stale(&entry, 1_060_000_000_000, max_age));

    // Stale
    assert!(is_stale(&entry, 1_060_000_000_001, max_age));
    assert!(is_stale(&entry, 2_000_000_000_000, max_age));
}

#[test]
fn test_compute_escrow_stats() {
    let entries: Vec<EscrowEntry> = vec![
        make_entry(1, 100, 1000),
        make_entry(2, 200, 2000),
        make_entry(3, 300, 3000),
    ];

    let stats = compute_escrow_stats(&entries);
    assert_eq!(stats.count, 3);
    assert_eq!(stats.total_shares, 600);
    assert_eq!(stats.total_expected_assets, 6000);
}

#[test]
fn test_find_by_owner() {
    let entries: Vec<EscrowEntry> = vec![make_entry(1, 100, 1000), make_entry(2, 200, 2000)];

    let found = find_by_owner(&entries, &owner_addr(2));
    assert!(found.is_some());
    assert_eq!(found.unwrap().shares, 200);

    let not_found = find_by_owner(&entries, &owner_addr(3));
    assert!(not_found.is_none());
}

#[test]
fn test_total_burn_and_refund() {
    let settlements: Vec<EscrowSettlement> = vec![
        EscrowSettlement::partial(50, 10),
        EscrowSettlement::partial(30, 20),
        EscrowSettlement::burn_all(100),
    ];

    assert_eq!(total_burn(&settlements), 180);
    assert_eq!(total_refund(&settlements), 30);
}

use proptest::prelude::*;

fn arb_entry() -> impl Strategy<Value = EscrowEntry> {
    (
        1u32..1000u32,
        0u128..=u64::MAX as u128,
        0u64..u64::MAX,
        0u128..=u64::MAX as u128,
    )
        .prop_map(|(owner_idx, shares, ts, expected)| {
            EscrowEntry::new(owner_addr(owner_idx as u64), shares, ts, expected)
        })
}

fn arb_entries(max_len: usize) -> impl Strategy<Value = Vec<EscrowEntry>> {
    proptest::collection::vec(arb_entry(), 0..=max_len)
}

proptest! {
    #[test]
    fn settle_proportional_conserves_shares(
        shares in 0u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
        actual_assets in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            expected_assets,
        );
        let settlement = settle_proportional(&entry, actual_assets);
        let total = settlement.to_burn.saturating_add(settlement.refund);
        prop_assert_eq!(total, shares);
    }

    #[test]
    fn settle_proportional_full_burn(
        shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
        extra in 0u128..=1_000_000u128,
    ) {
        let actual_assets = expected_assets.saturating_add(extra);
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            expected_assets,
        );
        let settlement = settle_proportional(&entry, actual_assets);

        prop_assert_eq!(settlement.to_burn, shares);
        prop_assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn settle_proportional_full_refund(
        shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            expected_assets,
        );
        let settlement = settle_proportional(&entry, 0);

        prop_assert_eq!(settlement.to_burn, 0);
        prop_assert_eq!(settlement.refund, shares);
    }

    #[test]
    fn settle_full_burn_burns_all(
        shares in 0u128..=u64::MAX as u128,
        expected_assets in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            expected_assets,
        );
        let settlement = EscrowSettlement::burn_all(entry.shares);

        prop_assert_eq!(settlement.to_burn, shares);
        prop_assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn settle_full_refund_refunds_all(
        shares in 0u128..=u64::MAX as u128,
        expected_assets in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            expected_assets,
        );
        let settlement = EscrowSettlement::refund_all(entry.shares);

        prop_assert_eq!(settlement.to_burn, 0);
        prop_assert_eq!(settlement.refund, shares);
    }

    #[test]
    fn apply_settlement_valid(
        shares in 1u128..=u64::MAX as u128,
        burn_ratio in 0u8..=100u8,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            1000,
        );
        let to_burn = (shares as u128 * burn_ratio as u128) / 100;
        let refund = shares - to_burn;
        let settlement = EscrowSettlement::partial(to_burn, refund);

        let result = apply_settlement(&entry, &settlement);
        prop_assert!(result.is_some());

        let result = result.unwrap();
        prop_assert_eq!(result.burned, to_burn);
        prop_assert_eq!(result.refunded, refund);
        prop_assert_eq!(result.remaining, 0);
    }

    #[test]
    fn apply_settlement_invalid(
        shares in 1u128..=u64::MAX as u128 - 1,
        excess in 1u128..=1_000_000u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            1000,
        );
        let settlement = EscrowSettlement::partial(shares, excess);

        let result = apply_settlement(&entry, &settlement);
        prop_assert!(result.is_none());
    }

    #[test]
    fn can_apply_settlement_consistency(
        shares in 0u128..=u64::MAX as u128,
        to_burn in 0u128..=u64::MAX as u128 / 2,
        refund in 0u128..=u64::MAX as u128 / 2,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            1000,
        );
        let settlement = EscrowSettlement::partial(to_burn, refund);
        let total = to_burn.saturating_add(refund);

        let can = can_apply_settlement(&entry, &settlement);
        prop_assert_eq!(can, total <= shares);
    }

    #[test]
    fn is_stale_consistency(
        created_at in 0u64..=u64::MAX / 2,
        max_age in 0u64..=u64::MAX / 4,
        delta in 0u64..=u64::MAX / 4,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            100,
            created_at,
            1000,
        );
        let now = created_at.saturating_add(delta);
        let threshold = created_at.saturating_add(max_age);

        let stale = is_stale(&entry, now, max_age);
        prop_assert_eq!(stale, now > threshold);
    }

    #[test]
    fn compute_escrow_stats_correct(
        entries in arb_entries(20),
    ) {
        let stats = compute_escrow_stats(&entries);

        let expected_count = entries.len() as u32;
        let expected_shares: u128 = entries.iter().map(|e| e.shares).sum();
        let expected_assets: u128 = entries.iter().map(|e| e.expected_assets).sum();

        prop_assert_eq!(stats.count, expected_count);
        prop_assert_eq!(stats.total_shares, expected_shares);
        prop_assert_eq!(stats.total_expected_assets, expected_assets);
    }

    #[test]
    fn total_burn_correct(
        settlements in proptest::collection::vec(
            (0u128..=u64::MAX as u128, 0u128..=u64::MAX as u128)
                .prop_map(|(b, r)| EscrowSettlement::partial(b, r)),
            0..20
        ),
    ) {
        let result = total_burn(&settlements);
        let expected: u128 = settlements.iter().map(|s| s.to_burn).sum();
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn total_refund_correct(
        settlements in proptest::collection::vec(
            (0u128..=u64::MAX as u128, 0u128..=u64::MAX as u128)
                .prop_map(|(b, r)| EscrowSettlement::partial(b, r)),
            0..20
        ),
    ) {
        let result = total_refund(&settlements);
        let expected: u128 = settlements.iter().map(|s| s.refund).sum();
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn entry_is_empty_consistency(
        shares in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(
            owner_addr(1),
            shares,
            0,
            1000,
        );
        prop_assert_eq!(entry.is_empty(), shares == 0);
    }

    #[test]
    fn burn_all_consistency(shares in 0u128..=u64::MAX as u128) {
        let s1 = EscrowSettlement::burn_all(shares);
        let s2 = EscrowSettlement::partial(shares, 0);
        prop_assert_eq!(s1.to_burn, s2.to_burn);
        prop_assert_eq!(s1.refund, s2.refund);
    }

    #[test]
    fn refund_all_consistency(shares in 0u128..=u64::MAX as u128) {
        let s1 = EscrowSettlement::refund_all(shares);
        let s2 = EscrowSettlement::partial(0, shares);
        prop_assert_eq!(s1.to_burn, s2.to_burn);
        prop_assert_eq!(s1.refund, s2.refund);
    }
}
