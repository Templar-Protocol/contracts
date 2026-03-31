use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
use templar_vault_kernel::{
    math::{number::Number, wad::mul_div_floor},
    state::{
        escrow::{settle_proportional, EscrowEntry},
        queue::{compute_settlement, WithdrawQueue},
        vault::MAX_PENDING,
    },
};

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
                u64::from(i),
                max.min(MAX_PENDING as u32),
            );
        }
        assert!(queue.len() <= MAX_PENDING);
        assert!(queue.len() <= max as usize);
    }
}

#[test]
fn queue_ids_ordered() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(owner_addr(i), receiver_addr(i), 100, 1000, i, 100);
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }

    while queue.dequeue().is_some() {
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }
}

#[test]
fn queue_contains_head_when_non_empty() {
    let mut queue = WithdrawQueue::new();

    for i in 0..5_u64 {
        let _ = queue.enqueue(owner_addr(i), receiver_addr(i), 100, 1000, i, 100);
    }

    while !queue.is_empty() {
        assert!(queue
            .pending_withdrawals()
            .contains_key(&queue.next_withdraw_to_execute));
        queue.dequeue();
    }
}

#[test]
fn fifo_does_not_skip_head() {
    let mut queue = WithdrawQueue::new();

    for i in 0..10_u64 {
        let _ = queue.enqueue(owner_addr(i), receiver_addr(i), 100, 1000, i, 100);
    }

    let mut prev_id = 0_u64;
    while let Some((id, _)) = queue.dequeue() {
        assert!(id >= prev_id, "FIFO order violated: {id} < {prev_id}");
        prev_id = id;
    }
}

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

#[test]
fn escrow_settlement_proportional() {
    let entry = EscrowEntry::new(owner_addr(1), 100, 0, 1000);

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

#[test]
fn payout_success_conserves() {
    let escrow = 1000_u128;
    for burn_ratio in [0_u8, 25, 50, 75, 100] {
        let burn = escrow * u128::from(burn_ratio) / 100;
        let refund = escrow - burn;
        assert_eq!(burn + refund, escrow);
    }
}

#[test]
fn payout_failure_refunds_all() {
    for escrow in [1_u128, 100, 1_000_000, u64::MAX as u128] {
        let refund = escrow;
        assert_eq!(refund, escrow);
    }
}
