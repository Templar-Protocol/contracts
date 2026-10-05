// Profile gating: the epoch settlement regression suite compiles only with
// `action-epoch-settlement` enabled. With the feature off, the restored
// origin/dev immediate settlement regression suite compiles instead.
#[cfg(feature = "action-epoch-settlement")]
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
#[cfg(feature = "action-epoch-settlement")]
use templar_vault_kernel::{
    math::{number::Number, wad::mul_div_floor},
    state::{
        escrow::{
            apply_settlement, can_apply_settlement, EscrowEntry, EscrowSettlement,
            SettlementResult,
        },
        queue::{compute_settlement, WithdrawQueue},
        settlement::EpochId,
        vault::MAX_PENDING,
    },
    TimestampNs,
};

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn queue_len_bounded() {
    for max in [1_u32, 10, 100, 1024] {
        let mut queue = WithdrawQueue::new();
        for i in 0..max + 10 {
            let _ = queue.enqueue(
                owner_addr(u64::from(i)),
                receiver_addr(u64::from(i)),
                100,
                0,
                TimestampNs(u64::from(i)),
                EpochId::FIRST_SETTLEMENT,
                max.min(MAX_PENDING as u32),
            );
        }
        assert!(queue.len() <= MAX_PENDING);
        assert!(queue.len() <= max as usize);
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn queue_ids_ordered() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            0,
            TimestampNs(i),
            EpochId::FIRST_SETTLEMENT,
            100,
        );
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }

    while queue.dequeue().is_some() {
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn queue_contains_head_when_non_empty() {
    let mut queue = WithdrawQueue::new();

    for i in 0..5_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            0,
            TimestampNs(i),
            EpochId::FIRST_SETTLEMENT,
            100,
        );
    }

    while !queue.is_empty() {
        assert!(queue
            .pending_withdrawals()
            .contains_key(&queue.next_withdraw_to_execute));
        queue.dequeue();
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn fifo_does_not_skip_head() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            0,
            TimestampNs(i),
            EpochId::FIRST_SETTLEMENT,
            100,
        );
    }

    let mut prev_id = 0_u64;
    while let Some((id, _)) = queue.dequeue() {
        assert!(id >= prev_id, "FIFO order violated: {id} < {prev_id}");
        prev_id = id;
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn no_shares_from_nothing() {
    let test_cases = [
        (0_u128, 1_u128, 1_u128),
        (0_u128, 1_000_000_u128, 1_000_000_u128),
        (0_u128, u64::MAX as u128, u64::MAX as u128),
    ];

    for (assets_in, total_supply, total_assets) in test_cases {
        let shares = mul_div_floor(
            Number::from(assets_in),
            Number::from(total_supply),
            Number::from(total_assets),
        );
        assert!(shares.is_zero(), "shares minted from zero assets");
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn positive_assets_mint_shares() {
    let test_cases = [
        (1_u128, 1_u128, 1_u128),
        (100_u128, 1000_u128, 1000_u128),
        (1_000_000_u128, 1_000_000_u128, 1_000_000_u128),
    ];

    for (assets_in, total_supply, total_assets) in test_cases {
        if total_supply > 0 && total_assets > 0 {
            let shares = mul_div_floor(
                Number::from(assets_in),
                Number::from(total_supply),
                Number::from(total_assets),
            );
            assert!(
                !shares.is_zero(),
                "no shares minted from positive assets: {assets_in} * {total_supply} / {total_assets}"
            );
        }
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn total_assets_accounting() {
    let test_cases = [
        (0_u128, 0_u128),
        (100_u128, 200_u128),
        (u64::MAX as u128 / 2, u64::MAX as u128 / 2),
    ];

    for (idle, external) in test_cases {
        let total = idle.saturating_add(external);
        assert_eq!(total, idle + external);
    }
}

/// A payout settlement must consume the escrow behind it exactly, whatever
/// assets were actually collected against the settled claim.
#[cfg(feature = "action-epoch-settlement")]
#[test]
fn settlement_conserves_shares() {
    let test_cases = [
        (100_u128, 1000_u128, 500_u128),
        (100_u128, 1000_u128, 1000_u128),
        (100_u128, 1000_u128, 0_u128),
        (100_u128, 1000_u128, 2000_u128),
    ];

    for (shares, settled_claim, actual) in test_cases {
        let settlement = compute_settlement(shares, settled_claim, actual);
        let total = settlement.to_burn.checked_add(settlement.refund);
        assert_eq!(
            total,
            Some(shares),
            "settlement does not conserve: burn={} + refund={} != {}",
            settlement.to_burn,
            settlement.refund,
            shares
        );

        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0));
        assert!(
            can_apply_settlement(&entry, &settlement),
            "settlement must consume every escrowed share"
        );
        assert!(apply_settlement(&entry, &settlement).is_some());
    }
}

#[cfg(feature = "action-epoch-settlement")]
#[test]
fn settlement_over_collection_burns_all_shares() {
    let settlement = compute_settlement(100_u128, 1000_u128, 1001_u128);

    assert_eq!(
        settlement.to_burn, 100,
        "over-collection should burn all shares"
    );
    assert_eq!(
        settlement.refund, 0,
        "over-collection should refund no shares"
    );
}

/// Escrow is only released when burn and refund together account for every
/// escrowed share. Short settlements strand escrow and are rejected.
#[cfg(feature = "action-epoch-settlement")]
#[test]
fn escrow_settlement_requires_exact_consumption() {
    let entry = EscrowEntry::new(owner_addr(1), 100, TimestampNs(0));

    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::partial(50, 50)),
        Some(SettlementResult {
            burned: 50,
            refunded: 50
        })
    );

    // A shortfall burn that leaves escrow behind is rejected outright.
    for (burned, refunded) in [(50_u128, 0_u128), (50, 49), (50, 51), (0, 0)] {
        let settlement = EscrowSettlement::partial(burned, refunded);
        assert!(
            !can_apply_settlement(&entry, &settlement),
            "{burned} + {refunded} must not settle 100 escrowed shares"
        );
        assert_eq!(apply_settlement(&entry, &settlement), None);
    }

    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::burn_all(101)
    ));
    assert!(!can_apply_settlement(
        &entry,
        &EscrowSettlement::refund_all(101)
    ));
}

/// Cancelling an unsettled request releases the entire escrow and burns
/// nothing.
#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_failure_refunds_all() {
    for escrow in [1_u128, 100, 1_000_000, u64::MAX as u128] {
        let entry = EscrowEntry::new(owner_addr(1), escrow, TimestampNs(0));
        let settlement = EscrowSettlement::refund_all(escrow);

        assert!(can_apply_settlement(&entry, &settlement));
        assert_eq!(
            apply_settlement(&entry, &settlement),
            Some(SettlementResult {
                burned: 0,
                refunded: escrow
            })
        );
    }
}

/// A successful payout burns the whole escrow; any other split must still
/// account for every escrowed share.
#[cfg(feature = "action-epoch-settlement")]
#[test]
fn payout_success_conserves() {
    let escrow = 1000_u128;
    for burn_ratio in [0_u8, 25, 50, 75, 100] {
        let burn = escrow * u128::from(burn_ratio) / 100;
        let refund = escrow - burn;
        let entry = EscrowEntry::new(owner_addr(1), escrow, TimestampNs(0));
        let settlement = EscrowSettlement::partial(burn, refund);

        assert_eq!(burn + refund, escrow);
        assert!(can_apply_settlement(&entry, &settlement));
        assert_eq!(
            apply_settlement(&entry, &settlement),
            Some(SettlementResult {
                burned: burn,
                refunded: refund
            })
        );
    }

    let entry = EscrowEntry::new(owner_addr(1), escrow, TimestampNs(0));
    assert_eq!(
        apply_settlement(&entry, &EscrowSettlement::burn_all(escrow)),
        Some(SettlementResult {
            burned: escrow,
            refunded: 0
        })
    );
}

#[cfg(not(feature = "action-epoch-settlement"))]
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
#[cfg(not(feature = "action-epoch-settlement"))]
use templar_vault_kernel::{
    math::{number::Number, wad::mul_div_floor},
    state::{
        escrow::{settle_proportional, EscrowEntry},
        queue::{compute_settlement, WithdrawQueue},
        vault::MAX_PENDING,
    },
    TimestampNs,
};

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn queue_len_bounded() {
    for max in [1_u32, 10, 100, 1024] {
        let mut queue = WithdrawQueue::new();
        for i in 0..max + 10 {
            let _ = queue.enqueue(
                owner_addr(u64::from(i)),
                receiver_addr(u64::from(i)),
                100,
                1000,
                TimestampNs(u64::from(i)),
                max.min(MAX_PENDING as u32),
            );
        }
        assert!(queue.len() <= MAX_PENDING);
        assert!(queue.len() <= max as usize);
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn queue_ids_ordered() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            1000,
            TimestampNs(i),
            100,
        );
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }

    while queue.dequeue().is_some() {
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn queue_contains_head_when_non_empty() {
    let mut queue = WithdrawQueue::new();

    for i in 0..5_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            1000,
            TimestampNs(i),
            100,
        );
    }

    while !queue.is_empty() {
        assert!(queue
            .pending_withdrawals()
            .contains_key(&queue.next_withdraw_to_execute));
        queue.dequeue();
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn fifo_does_not_skip_head() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(
            owner_addr(i),
            receiver_addr(i),
            100,
            1000,
            TimestampNs(i),
            100,
        );
    }

    let mut prev_id = 0_u64;
    while let Some((id, _)) = queue.dequeue() {
        assert!(id >= prev_id, "FIFO order violated: {id} < {prev_id}");
        prev_id = id;
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn no_shares_from_nothing() {
    let test_cases = [
        (0_u128, 1_u128, 1_u128),
        (0_u128, 1_000_000_u128, 1_000_000_u128),
        (0_u128, u64::MAX as u128, u64::MAX as u128),
    ];

    for (assets_in, total_supply, total_assets) in test_cases {
        let shares = mul_div_floor(
            Number::from(assets_in),
            Number::from(total_supply),
            Number::from(total_assets),
        );
        assert!(shares.is_zero(), "shares minted from zero assets");
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn positive_assets_mint_shares() {
    let test_cases = [
        (1_u128, 1_u128, 1_u128),
        (100_u128, 1000_u128, 1000_u128),
        (1_000_000_u128, 1_000_000_u128, 1_000_000_u128),
    ];

    for (assets_in, total_supply, total_assets) in test_cases {
        if total_supply > 0 && total_assets > 0 {
            let shares = mul_div_floor(
                Number::from(assets_in),
                Number::from(total_supply),
                Number::from(total_assets),
            );
            assert!(
                !shares.is_zero(),
                "no shares minted from positive assets: {assets_in} * {total_supply} / {total_assets}"
            );
        }
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn total_assets_accounting() {
    let test_cases = [
        (0_u128, 0_u128),
        (100_u128, 200_u128),
        (u64::MAX as u128 / 2, u64::MAX as u128 / 2),
    ];

    for (idle, external) in test_cases {
        let total = idle.saturating_add(external);
        assert_eq!(total, idle + external);
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settlement_conserves_shares() {
    let test_cases = [
        (100_u128, 1000_u128, 500_u128),
        (100_u128, 1000_u128, 1000_u128),
        (100_u128, 1000_u128, 0_u128),
        (100_u128, 1000_u128, 2000_u128),
    ];

    for (shares, expected, actual) in test_cases {
        let settlement = compute_settlement(shares, expected, actual);
        let total = settlement.to_burn.saturating_add(settlement.refund);
        assert_eq!(
            total, shares,
            "settlement does not conserve: burn={} + refund={} != {}",
            settlement.to_burn, settlement.refund, shares
        );
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn settlement_over_collection_burns_all_shares() {
    let settlement = compute_settlement(100_u128, 1000_u128, 1001_u128);

    assert_eq!(
        settlement.to_burn, 100,
        "over-collection should burn all shares"
    );
    assert_eq!(
        settlement.refund, 0,
        "over-collection should refund no shares"
    );
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn escrow_settlement_proportional() {
    let entry = EscrowEntry::new(owner_addr(1), 100, TimestampNs(0), 1000);

    let half = settle_proportional(&entry, 500);
    assert_eq!(half.to_burn + half.refund, 100);
    assert_eq!(half.to_burn, 50);

    let zero = settle_proportional(&entry, 0);
    assert_eq!(zero.to_burn, 0);
    assert_eq!(zero.refund, 100);

    let full = settle_proportional(&entry, 1000);
    assert_eq!(full.to_burn, 100);
    assert_eq!(full.refund, 0);

    let above_full = settle_proportional(&entry, 2000);
    assert_eq!(above_full.to_burn, 100);
    assert_eq!(above_full.refund, 0);
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn payout_success_conserves() {
    let escrow = 1000_u128;
    for burn_ratio in [0_u8, 25, 50, 75, 100] {
        let burn = escrow * u128::from(burn_ratio) / 100;
        let refund = escrow - burn;
        assert_eq!(burn + refund, escrow);
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[test]
fn payout_failure_refunds_all() {
    for escrow in [1_u128, 100, 1_000_000, u64::MAX as u128] {
        let refund = escrow;
        assert_eq!(refund, escrow);
    }
}
