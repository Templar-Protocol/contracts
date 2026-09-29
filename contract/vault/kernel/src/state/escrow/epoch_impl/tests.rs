use super::*;
use crate::state::queue::{
    compute_full_withdrawal, compute_idle_settlement, compute_partial_withdrawal,
    PendingWithdrawal,
};
use crate::state::settlement::{EpochId, EpochState, ValuationReportRef};
use crate::test_utils::owner_addr;
use alloc::vec;
use alloc::vec::Vec;

fn make_entry(owner: u64, shares: u128) -> EscrowEntry {
    EscrowEntry::new(
        owner_addr(owner),
        shares,
        TimestampNs(1_000_000_000_000), // 1 second in ns
    )
}

/// Build settled epoch state whose accepted snapshot values the vault at
/// `settlement_nav` against `eligible_supply`, so every claim below is derived
/// from settlement rather than quoted at request time.
fn settled_epoch_state(settlement_nav: u128, eligible_supply: u128) -> EpochState {
    let cutoff = TimestampNs(2_000_000_000_000);
    let report = ValuationReportRef {
        report_seq: 1,
        as_of_ns: cutoff,
        report_hash: [7u8; 32],
    };
    let cutoff_state = EpochState::genesis()
        .begin_cutoff(cutoff)
        .expect("epoch cutoff accepted");
    let snapshot = cutoff_state
        .build_settlement_snapshot(
            &report,
            settlement_nav,
            eligible_supply,
            cutoff,
            60_000_000_000,
        )
        .expect("accepted report settles the epoch");
    let settled = cutoff_state
        .apply_settled(&snapshot)
        .expect("settlement recorded");
    assert!(settled.check_invariants());
    settled
}

/// Request record used to exercise execution-time settlement math.
fn withdrawal(shares: u128) -> PendingWithdrawal {
    PendingWithdrawal::new(
        owner_addr(1),
        owner_addr(1),
        shares,
        0,
        TimestampNs(0),
        EpochId::FIRST_SETTLEMENT,
    )
    .expect("valid unpriced request")
}

#[test]
fn test_escrow_entry_is_empty() {
    let entry = make_entry(1, 0);
    assert!(entry.is_empty());

    let entry = make_entry(1, 100);
    assert!(!entry.is_empty());
}

#[test]
fn test_apply_settlement_requires_exact_consumption() {
    let entry = make_entry(1, 100);
    let settlement = EscrowSettlement::partial(60, 40);

    assert_eq!(
        apply_settlement(&entry, &settlement),
        Some(SettlementResult {
            burned: 60,
            refunded: 40
        })
    );
}

#[test]
fn test_apply_settlement_full_burn() {
    let entry = make_entry(1, 100);

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::burn_all(100)),
        Some(SettlementResult {
            burned: 100,
            refunded: 0
        })
    );
}

#[test]
fn test_apply_settlement_full_refund() {
    let entry = make_entry(1, 100);

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::refund_all(100)),
        Some(SettlementResult {
            burned: 0,
            refunded: 100
        })
    );
}

/// The settlement helpers are pure arithmetic validators: they price a
/// settlement against an entry and never mutate it. Double settlement is
/// prevented by the request lifecycle: cancellation consumes the queued
/// request and repairs the FIFO caches, after which no re-settlement or
/// re-cancellation can find it.
#[test]
fn settled_refund_cannot_be_replayed_after_queue_consumption() {
    let mut queue = crate::state::queue::WithdrawQueue::new();
    let request_id = queue
        .enqueue_withdrawal(withdrawal(100), 16)
        .expect("request enqueued");
    let entry = make_entry(1, 100);

    let cancelled = apply_settlement(&entry, &EscrowSettlement::refund_all(100))
        .expect("law-compliant full refund is priced");
    assert_eq!(
        cancelled,
        SettlementResult {
            burned: 0,
            refunded: 100
        }
    );
    assert_eq!(queue.remove_pending(request_id).map(|w| w.escrow_shares), Some(100));
    assert!(queue.is_empty());
    assert_eq!(queue.remove_pending(request_id), None);
    assert!(queue.check_invariants());

    // Every burn/refund partition consumes the escrow exactly once and is
    // payable. Any attempt to burn while stranding shares is rejected by
    // the exact-partition law.
    for burned in [0u128, 1, 40, 60, 99, 100] {
        let settlement = EscrowSettlement::partial(burned, 100 - burned);
        assert!(
            can_apply_settlement(&entry, &settlement),
            "{burned} + {} must consume the escrow exactly once",
            100 - burned
        );
        if burned > 0 {
            // Burn any shares while stranding the rest: the refund total
            // never completes the exact partition for any probed burn.
            let stranded = EscrowSettlement::partial(burned, 2);
            assert!(
                !can_apply_settlement(&entry, &stranded),
                "{burned} + 2 must not settle 100 escrowed shares"
            );
            assert_eq!(apply_settlement(&entry, &stranded), None);
        }
    }
}

#[test]
fn test_apply_settlement_rejects_under_settlement() {
    let entry = make_entry(1, 100);
    let settlement = EscrowSettlement::partial(30, 20);

    assert_eq!(apply_settlement(&entry, &settlement), None);
    assert!(!can_apply_settlement(&entry, &settlement));
}

#[test]
fn test_apply_settlement_rejects_zero_settlement() {
    let entry = make_entry(1, 100);

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::partial(0, 0)),
        None
    );
}

#[test]
fn test_apply_settlement_rejects_over_settlement() {
    let entry = make_entry(1, 100);
    let settlement = EscrowSettlement::partial(80, 30); // 110 > 100

    assert_eq!(apply_settlement(&entry, &settlement), None);
    assert!(!can_apply_settlement(&entry, &settlement));
}

#[test]
fn test_apply_settlement_overflow_rejected() {
    let entry = make_entry(1, u128::MAX);
    let settlement = EscrowSettlement::partial(u128::MAX, 1);

    assert_eq!(apply_settlement(&entry, &settlement), None);
    assert!(!can_apply_settlement(&entry, &settlement));
}

#[test]
fn test_apply_settlement_rejects_more_than_escrowed() {
    let entry = make_entry(1, 100);

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::burn_all(101)),
        None
    );
    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::refund_all(101)),
        None
    );
}

#[test]
fn test_empty_escrow_only_settles_empty() {
    let entry = make_entry(1, 0);

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::partial(0, 0)),
        Some(SettlementResult {
            burned: 0,
            refunded: 0
        })
    );
    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::partial(1, 0)),
        None
    );
}

#[test]
fn test_full_burn_and_refund_constructors() {
    let entry = make_entry(1, 100);

    let burn = EscrowSettlement::burn_all(entry.shares);
    assert_eq!(burn.to_burn, 100);
    assert_eq!(burn.refund, 0);

    let refund = EscrowSettlement::refund_all(entry.shares);
    assert_eq!(refund.to_burn, 0);
    assert_eq!(refund.refund, 100);
}

#[test]
fn test_can_apply_settlement() {
    let entry = make_entry(1, 100);

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

    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::partial(60, 50)
    ));
    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::partial(60, 39)
    ));
    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::burn_all(101)
    ));
}

#[test]
fn test_is_stale() {
    let entry = make_entry(1, 100);
    let max_age = 60_000_000_000u64; // 60 seconds

    // Not stale
    assert!(!is_stale(&entry, TimestampNs(1_000_000_000_000), max_age));
    assert!(!is_stale(&entry, TimestampNs(1_060_000_000_000), max_age));

    // Stale
    assert!(is_stale(&entry, TimestampNs(1_060_000_000_001), max_age));
    assert!(is_stale(&entry, TimestampNs(2_000_000_000_000), max_age));
}

#[test]
fn test_compute_escrow_stats_is_share_denominated() {
    let entries: Vec<EscrowEntry> = vec![
        make_entry(1, 100),
        make_entry(2, 200),
        make_entry(3, 300),
    ];

    let stats = compute_escrow_stats(&entries);
    assert_eq!(stats.count, 3);
    assert_eq!(stats.total_shares, 600);
}

#[test]
fn test_find_by_owner() {
    let entries: Vec<EscrowEntry> = vec![make_entry(1, 100), make_entry(2, 200)];

    let found = find_by_owner(&entries, &owner_addr(2));
    assert!(found.is_some());
    assert_eq!(found.unwrap().shares, 200);

    let not_found = find_by_owner(&entries, &owner_addr(3));
    assert!(not_found.is_none());
}

#[test]
fn test_total_burn_and_refund() {
    let settlements = vec![
        EscrowSettlement::partial(50, 10),
        EscrowSettlement::partial(30, 20),
        EscrowSettlement::burn_all(100),
    ];

    assert_eq!(total_burn(&settlements), Some(180));
    assert_eq!(total_refund(&settlements), Some(30));
}

#[test]
fn test_total_burn_and_refund_reject_overflow() {
    let burning = vec![
        EscrowSettlement::burn_all(u128::MAX),
        EscrowSettlement::burn_all(1),
    ];
    assert_eq!(total_burn(&burning), None);

    let refunding = vec![
        EscrowSettlement::refund_all(u128::MAX),
        EscrowSettlement::refund_all(1),
    ];
    assert_eq!(total_refund(&refunding), None);
}

#[test]
fn test_settled_claim_is_the_only_payout_figure() {
    // 10% of the epoch NAV is owed on 100 escrowed shares.
    let epoch_state = settled_epoch_state(100_000, 1_000);
    let shares = 100u128;

    assert_eq!(
        epoch_state.settled_claim_for(EpochId::FIRST_SETTLEMENT, shares),
        Some(10_000)
    );
    // An epoch the request was not queued in cannot pay it.
    assert_eq!(
        epoch_state.settled_claim_for(EpochId::new(7), shares),
        None
    );
    // Nothing is owed before settlement covers the epoch.
    assert_eq!(
        EpochState::genesis().settled_claim_for(EpochId::FIRST_SETTLEMENT, shares),
        None
    );
}

#[test]
fn test_fully_funded_payout_burns_all_escrow() {
    let epoch_state = settled_epoch_state(100_000, 1_000);
    let shares = 100u128;
    let entry = make_entry(1, shares);

    let result = compute_full_withdrawal(&withdrawal(shares), &epoch_state, 10_000)
        .expect("claim is fully payable");
    assert_eq!(result.assets_out, 10_000);
    assert_eq!(result.settlement.to_burn, shares);
    assert_eq!(result.settlement.refund, 0);
    assert_eq!(
        apply_settlement(&entry, &result.settlement),
        Some(SettlementResult {
            burned: shares,
            refunded: 0
        })
    );
}

#[test]
fn test_unsettled_epoch_cannot_pay_escrow() {
    let shares = 100u128;

    assert_eq!(
        compute_full_withdrawal(&withdrawal(shares), &EpochState::genesis(), u128::MAX),
        None
    );
    // Nothing was burned, so the escrow stays whole and refundable.
    assert_eq!(
        apply_settlement(
            &make_entry(1, shares),
            &EscrowSettlement::refund_all(shares)
        )
        .map(|r| r.refunded),
        Some(shares)
    );
}

#[test]
fn test_partial_payout_still_consumes_all_escrow() {
    let epoch_state = settled_epoch_state(100_000, 1_000);
    let shares = 100u128;
    let entry = make_entry(1, shares);

    let partial = compute_partial_withdrawal(&withdrawal(shares), &epoch_state, 5_000);
    assert!(partial.assets_out > 0);
    assert!(partial.assets_out < 10_000);
    assert_eq!(
        partial.settlement.to_burn + partial.settlement.refund,
        shares
    );
    assert_eq!(
        apply_settlement(&entry, &partial.settlement).map(|r| r.burned + r.refunded),
        Some(shares)
    );

    // Burning the redeemed amount while stranding the remainder is rejected,
    // even though the burn alone matches the partial payout.
    let stranded = EscrowSettlement::partial(partial.settlement.to_burn, 0);
    assert!(!can_apply_settlement(&entry, &stranded));
    assert_eq!(apply_settlement(&entry, &stranded), None);
}

#[test]
fn test_unpayable_settlement_refunds_all_escrow() {
    let shares = 100u128;
    let entry = make_entry(1, shares);

    let result = compute_idle_settlement(shares, 0, 0).expect("cancellation settles");
    assert_eq!(result.assets_out, 0);
    assert_eq!(result.settlement.to_burn, 0);
    assert_eq!(result.settlement.refund, shares);
    assert_eq!(
        apply_settlement(&entry, &result.settlement),
        Some(SettlementResult {
            burned: 0,
            refunded: shares
        })
    );
}

use proptest::prelude::*;

fn arb_entry() -> impl Strategy<Value = EscrowEntry> {
    (1u32..1000u32, 0u128..=u64::MAX as u128, 0u64..u64::MAX).prop_map(
        |(owner_idx, shares, ts)| {
            EscrowEntry::new(owner_addr(owner_idx as u64), shares, TimestampNs(ts))
        },
    )
}

fn arb_entries(max_len: usize) -> impl Strategy<Value = Vec<EscrowEntry>> {
    proptest::collection::vec(arb_entry(), 0..=max_len)
}

proptest! {
    #[test]
    fn apply_settlement_accepts_every_exact_partition(
        shares in 1u128..=u64::MAX as u128,
        burn_ratio in 0u8..=100u8,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        let to_burn = (shares * burn_ratio as u128) / 100;
        let refund = shares - to_burn;
        let settlement = EscrowSettlement::partial(to_burn, refund);

        let result = apply_settlement(&entry, &settlement).expect("exact settlement applies");
        prop_assert_eq!(result.burned, to_burn);
        prop_assert_eq!(result.refunded, refund);
        prop_assert_eq!(result.burned + result.refunded, entry.shares);
    }

    #[test]
    fn apply_settlement_rejects_any_imperfect_partition(
        shares in 1u128..=u64::MAX as u128 / 4,
        to_burn in 0u128..=u64::MAX as u128 / 2,
        refund in 0u128..=u64::MAX as u128 / 2,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        let settlement = EscrowSettlement::partial(to_burn, refund);

        if to_burn + refund == shares {
            let result = apply_settlement(&entry, &settlement).expect("exact settlement applies");
            prop_assert_eq!(result.burned + result.refunded, shares);
        } else {
            prop_assert_eq!(apply_settlement(&entry, &settlement), None);
            prop_assert!(!can_apply_settlement(&entry, &settlement));
        }
    }

    #[test]
    fn can_apply_settlement_agrees_with_apply(
        shares in 0u128..=u64::MAX as u128 / 2,
        to_burn in 0u128..=u64::MAX as u128 / 2,
        refund in 0u128..=u64::MAX as u128 / 2,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        let settlement = EscrowSettlement::partial(to_burn, refund);

        prop_assert_eq!(
            can_apply_settlement(&entry, &settlement),
            apply_settlement(&entry, &settlement).is_some()
        );
    }

    #[test]
    fn exact_settlements_never_strand_escrow(
        entries in arb_entries(20),
        burn_ratios in proptest::collection::vec(0u8..=100u8, 1..=20),
    ) {
        let settlements: Vec<EscrowSettlement> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let ratio = burn_ratios[index % burn_ratios.len()];
                let to_burn = (entry.shares * ratio as u128) / 100;
                EscrowSettlement::partial(to_burn, entry.shares - to_burn)
            })
            .collect();

        let burned = total_burn(&settlements).expect("checked burn total");
        let refunded = total_refund(&settlements).expect("checked refund total");
        let stats = compute_escrow_stats(&entries);

        prop_assert_eq!(burned + refunded, stats.total_shares);
        for settlement in &settlements {
            prop_assert!(settlement.to_burn + settlement.refund <= stats.total_shares);
        }
    }

    #[test]
    fn over_settlement_always_rejected(
        shares in 1u128..=u64::MAX as u128 - 1,
        excess in 1u128..=1_000_000u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        let settlement = EscrowSettlement::partial(shares, excess);

        prop_assert_eq!(apply_settlement(&entry, &settlement), None);
        prop_assert!(!can_apply_settlement(&entry, &settlement));
    }

    #[test]
    fn is_stale_consistency(
        created_at in 0u64..=u64::MAX / 2,
        max_age in 0u64..=u64::MAX / 4,
        delta in 0u64..=u64::MAX / 4,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), 100, TimestampNs(created_at));
        let now = created_at.saturating_add(delta);
        let threshold = created_at.saturating_add(max_age);

        let stale = is_stale(&entry, TimestampNs(now), max_age);
        prop_assert_eq!(stale, now > threshold);
    }

    #[test]
    fn compute_escrow_stats_correct(
        entries in arb_entries(20),
    ) {
        let stats = compute_escrow_stats(&entries);

        let expected_count = entries.len() as u32;
        let expected_shares: u128 = entries.iter().map(|e| e.shares).sum();

        prop_assert_eq!(stats.count, expected_count);
        prop_assert_eq!(stats.total_shares, expected_shares);
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
        prop_assert_eq!(result, Some(expected));
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
        prop_assert_eq!(result, Some(expected));
    }

    #[test]
    fn entry_is_empty_consistency(
        shares in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
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

    #[test]
    fn epoch_claims_are_proportional_and_never_overstate(
        shares in 0u128..=1_000_000u128,
        settlement_nav in 1u128..=1_000_000_000u128,
        supply_factor in 1u128..=1_000u128,
    ) {
        // Full escrow can never redeem more than the epoch's total assets.
        let eligible_supply = (shares + 1).saturating_mul(supply_factor);
        let epoch_state = settled_epoch_state(settlement_nav, eligible_supply);
        let claim = epoch_state
            .settled_claim_for(EpochId::FIRST_SETTLEMENT, shares)
            .expect("settled claim");
        prop_assert!(claim <= settlement_nav);

        // Other epochs stay inert, and escrow conservation still holds.
        prop_assert_eq!(
            epoch_state.settled_claim_for(EpochId::new(9), shares),
            None
        );
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        let burned = apply_settlement(&entry, &EscrowSettlement::burn_all(shares));
        let refunded = apply_settlement(&entry, &EscrowSettlement::refund_all(shares));
        prop_assert_eq!(burned.as_ref().map(|r| r.burned + r.refunded), Some(shares));
        prop_assert_eq!(refunded.as_ref().map(|r| r.burned + r.refunded), Some(shares));
    }

    #[test]
    fn derived_payout_settlements_always_consume_escrow(
        shares in 1u128..=1_000_000u128,
        settlement_nav in 1u128..=1_000_000_000u128,
        available_ratio in 0u8..=100u8,
    ) {
        let eligible_supply = 1_000u128;
        let epoch_state = settled_epoch_state(settlement_nav, eligible_supply);
        let claim = epoch_state
            .settled_claim_for(EpochId::FIRST_SETTLEMENT, shares)
            .expect("settled claim");
        let available = (claim as u128 * available_ratio as u128) / 100;

        let result = compute_partial_withdrawal(&withdrawal(shares), &epoch_state, available);
        prop_assert!(result.assets_out <= claim);
        prop_assert_eq!(
            result.settlement.to_burn + result.settlement.refund,
            shares
        );
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        prop_assert!(can_apply_settlement(&entry, &result.settlement));
    }
}
