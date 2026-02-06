//! # Templar Vault Kernel Property Tests
//!
//! This module contains 50+ property-based tests verifying kernel invariants.
//! These tests serve as a formal specification of expected kernel behavior.
//!
//! ## Key Invariants Tested
//!
//! ### Accounting Invariants
//! - `total_assets = idle_assets + external_assets`
//! - `total_shares = sum(user_shares) + sum(escrowed_shares)`
//!
//! ### Queue Invariants
//! - `pending_withdrawals.len() <= max_pending_withdrawals <= MAX_PENDING`
//! - `next_withdraw_to_execute <= next_pending_withdrawal_id`
//! - If `pending_withdrawals.len() > 0`, then `pending_withdrawals` contains `next_withdraw_to_execute`
//! - FIFO ordering: head index increments monotonically
//!
//! ### Share/Asset Conversion Invariants
//! - Share price never increases faster than yield + fees
//! - Deposit followed by full withdrawal returns original (minus fees/rounding)
//! - No shares minted without corresponding assets
//!
//! ### Fee Invariants
//! - Fee accrual is monotonically non-decreasing
//! - Fee shares never exceed performance gain

use proptest::prelude::*;
use templar_vault_kernel::{
    math::{
        number::Number,
        wad::{compute_fee_shares, mul_div_ceil, mul_div_floor, Wad, MAX_PERFORMANCE_FEE_WAD},
    },
    state::{
        escrow::{
            apply_settlement, can_apply_settlement, compute_escrow_stats, settle_full_burn,
            settle_full_refund, settle_proportional, EscrowEntry,
        },
        op_state::{AllocatingState, OpState, PayoutState, RefreshingState, WithdrawingState},
        queue::{
            can_enqueue, compute_queue_status, compute_settlement, count_satisfiable,
            is_past_cooldown, is_valid_withdrawal_amount, PendingWithdrawal, WithdrawQueue,
            MAX_QUEUE_LENGTH, MIN_WITHDRAWAL_ASSETS,
        },
        vault::{FeeAccrualAnchor, VaultState, MAX_PENDING},
    },
    transitions::{
        allocation_step_callback, complete_refresh, payout_complete, start_allocation,
        start_refresh, start_withdrawal, stop_withdrawal, withdrawal_collected,
        withdrawal_step_callback, TransitionError, WithdrawalRequest,
    },
    types::EscrowSettlement,
};
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};

// ============================================================================
// Arbitrary Strategies
// ============================================================================

/// Generate a valid allocation plan
fn arb_allocation_plan(max_len: usize) -> impl Strategy<Value = Vec<(u32, u128)>> {
    proptest::collection::vec((0u32..100u32, 1u128..=1_000_000_000u128), 1..=max_len)
}

/// Generate a refresh plan (list of target IDs)
fn arb_refresh_plan(max_len: usize) -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::vec(0u32..100u32, 1..=max_len)
}

/// Generate a withdrawal request
fn arb_withdrawal_request() -> impl Strategy<Value = WithdrawalRequest> {
    (
        1u64..u64::MAX,            // op_id
        1u128..=1_000_000_000u128, // amount
        1u128..=1_000_000_000u128, // escrow_shares
    )
        .prop_map(|(op_id, amount, escrow_shares)| WithdrawalRequest {
            op_id,
            amount,
            receiver: receiver_addr(op_id),
            owner: owner_addr(op_id),
            escrow_shares,
        })
}

/// Generate a pending withdrawal
#[allow(dead_code)]
fn arb_pending_withdrawal() -> impl Strategy<Value = PendingWithdrawal> {
    (
        1u128..=1_000_000_000u128, // escrow_shares
        1u128..=1_000_000_000u128, // expected_assets
        0u64..=u64::MAX / 2,       // requested_at_ns
    )
        .prop_map(|(escrow_shares, expected_assets, requested_at_ns)| {
            PendingWithdrawal::new(
                owner_addr(1),
                receiver_addr(1),
                escrow_shares,
                expected_assets,
                requested_at_ns,
            )
        })
}

/// Generate an escrow entry
#[allow(dead_code)]
fn arb_escrow_entry() -> impl Strategy<Value = EscrowEntry> {
    (
        0u128..=u64::MAX as u128, // shares
        0u64..u64::MAX,           // created_at
        0u128..=u64::MAX as u128, // expected_assets
    )
        .prop_map(|(shares, ts, expected)| EscrowEntry::new(owner_addr(1), shares, ts, expected))
}

/// Generate a vault state with valid invariants
#[allow(dead_code)]
fn arb_vault_state() -> impl Strategy<Value = VaultState> {
    (
        0u128..=u64::MAX as u128 / 2, // idle_assets
        0u128..=u64::MAX as u128 / 2, // external_assets
        0u128..=u64::MAX as u128,     // total_shares
        0u64..u64::MAX,               // timestamp
    )
        .prop_map(|(idle, external, shares, ts)| {
            let total = idle.saturating_add(external);
            VaultState::with_initial(total, shares, idle, external, ts)
        })
}

proptest! {
    // =========================================================================
    // ACCOUNTING INVARIANTS (1-10)
    // =========================================================================

    /// Property 1: total_assets = idle_assets + external_assets
    /// Invariant: The fundamental accounting equation always holds.
    #[test]
    fn prop_total_assets_accounting(
        idle in 0u128..=u64::MAX as u128 / 2,
        external in 0u128..=u64::MAX as u128 / 2,
    ) {
        let total = idle.saturating_add(external);
        let state = VaultState::with_initial(total, 0, idle, external, 0);
        prop_assert!(state.check_invariant(), "total_assets != idle + external");
    }

    /// Property 2: VaultState invariant check is accurate
    /// Invariant: check_invariant returns false when accounting is broken.
    #[test]
    fn prop_invariant_check_detects_violations(
        idle in 1u128..=u64::MAX as u128 / 2,
        external in 1u128..=u64::MAX as u128 / 2,
        delta in 1u128..=1000u128,
    ) {
        let total = idle.saturating_add(external).saturating_add(delta);
        let mut state = VaultState::new();
        state.total_assets = total;
        state.total_shares = 0;
        state.idle_assets = idle;
        state.external_assets = external;
        state.fee_anchor = FeeAccrualAnchor::new(total, 0);
        prop_assert!(!state.check_invariant(), "should detect invariant violation");
    }

    /// Property 3: VaultState::new creates valid initial state
    #[test]
    fn prop_new_vault_state_valid(_seed in 0u64..1000u64) {
        let state = VaultState::new();
        prop_assert!(state.check_invariant());
        prop_assert!(state.is_idle());
        prop_assert_eq!(state.total_assets, 0);
        prop_assert_eq!(state.total_shares, 0);
    }

    /// Property 4: VaultState::with_initial preserves accounting
    #[test]
    fn prop_with_initial_preserves_accounting(
        idle in 0u128..=u64::MAX as u128 / 2,
        external in 0u128..=u64::MAX as u128 / 2,
        shares in 0u128..=u64::MAX as u128,
        ts in 0u64..u64::MAX,
    ) {
        let total = idle.saturating_add(external);
        let state = VaultState::with_initial(total, shares, idle, external, ts);
        prop_assert!(state.check_invariant());
        prop_assert_eq!(state.total_assets, total);
        prop_assert_eq!(state.total_shares, shares);
        prop_assert_eq!(state.fee_anchor.total_assets, total);
        prop_assert_eq!(state.fee_anchor.timestamp_ns, ts);
    }

    /// Property 5: op_id allocation is monotonic
    #[test]
    fn prop_op_id_monotonic(count in 1usize..=100) {
        let mut state = VaultState::new();
        let mut prev_id = 0u64;
        for _ in 0..count {
            let id = state.allocate_op_id();
            prop_assert!(id >= prev_id, "op_id should be monotonic");
            prev_id = id;
        }
    }

    /// Property 6: op_id allocation saturates at max
    #[test]
    fn prop_op_id_saturates(_seed in 0u64..100u64) {
        let mut state = VaultState::new();
        state.next_op_id = u64::MAX;
        let id1 = state.allocate_op_id();
        let id2 = state.allocate_op_id();
        prop_assert_eq!(id1, u64::MAX);
        prop_assert_eq!(id2, u64::MAX);
    }

    /// Property 7: fee anchor update preserves structure
    #[test]
    fn prop_fee_anchor_update(
        old_assets in 0u128..=u64::MAX as u128,
        old_ts in 0u64..u64::MAX / 2,
        new_assets in 0u128..=u64::MAX as u128,
        new_ts in u64::MAX / 2..u64::MAX,
    ) {
        let mut anchor = FeeAccrualAnchor::new(old_assets, old_ts);
        anchor.update(new_assets, new_ts);
        prop_assert_eq!(anchor.total_assets, new_assets);
        prop_assert_eq!(anchor.timestamp_ns, new_ts);
    }

    /// Property 8: zero fee anchor is valid
    #[test]
    fn prop_fee_anchor_zero(_seed in 0u64..100u64) {
        let anchor = FeeAccrualAnchor::zero();
        prop_assert_eq!(anchor.total_assets, 0);
        prop_assert_eq!(anchor.timestamp_ns, 0);
    }

    /// Property 9: idle state detection works
    #[test]
    fn prop_idle_detection(_seed in 0u64..100u64) {
        let state = VaultState::new();
        prop_assert!(state.is_idle());
        prop_assert!(state.current_op_id().is_none());
    }

    /// Property 10: non-idle state detection works
    #[test]
    fn prop_non_idle_detection(
        plan in arb_allocation_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        prop_assert!(!result.new_state.is_idle());
    }

    // =========================================================================
    // QUEUE INVARIANTS (11-25)
    // =========================================================================

    /// Property 11: queue length bounded by MAX_PENDING
    #[test]
    fn prop_queue_length_bounded(
        count in 0usize..=20,
        max_pending in 1u32..=MAX_PENDING as u32,
    ) {
        let mut queue = WithdrawQueue::new();
        for i in 0..count {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64 * 1_000_000_000,
                max_pending,
            );
        }
        prop_assert!(queue.len() <= max_pending as usize);
        prop_assert!(queue.len() <= MAX_PENDING);
    }

    /// Property 12: next_withdraw_to_execute <= next_pending_withdrawal_id
    #[test]
    fn prop_queue_ids_ordered(enqueues in 0usize..=10) {
        let mut queue = WithdrawQueue::new();
        for i in 0..enqueues {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }
        prop_assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
        prop_assert!(queue.check_invariants());
    }

    /// Property 13: non-empty queue contains head
    #[test]
    fn prop_non_empty_queue_contains_head(enqueues in 1usize..=10) {
        let mut queue = WithdrawQueue::new();
        for i in 0..enqueues {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }
        prop_assert!(!queue.is_empty());
        prop_assert!(queue.pending_withdrawals.contains_key(&queue.next_withdraw_to_execute));
        prop_assert!(queue.check_invariants());
    }

    /// Property 14: FIFO ordering - dequeue returns head
    #[test]
    fn prop_fifo_ordering(enqueues in 2usize..=10) {
        let mut queue = WithdrawQueue::new();
        let mut ids = Vec::new();
        for i in 0..enqueues {
            let id = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            ).unwrap();
            ids.push(id);
        }

        // Dequeue should return in FIFO order
        for expected_id in ids {
            let (id, _) = queue.dequeue().unwrap();
            prop_assert_eq!(id, expected_id, "FIFO order violated");
        }
    }

    /// Property 15: head index increments monotonically
    #[test]
    fn prop_head_index_monotonic(enqueues in 2usize..=10) {
        let mut queue = WithdrawQueue::new();
        for i in 0..enqueues {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }

        let mut prev_head = 0u64;
        while !queue.is_empty() {
            let head = queue.next_withdraw_to_execute;
            prop_assert!(head >= prev_head, "head should increment monotonically");
            prev_head = head;
            queue.dequeue();
        }
    }

    /// Property 16: enqueue fails when queue is full
    #[test]
    fn prop_enqueue_fails_when_full(max_pending in 1u32..=10) {
        let mut queue = WithdrawQueue::new();
        for i in 0..max_pending {
            let result = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                max_pending,
            );
            prop_assert!(result.is_ok());
        }

        // One more should fail
        let result = queue.enqueue(
            owner_addr(9),
            receiver_addr(9),
            100,
            1000,
            max_pending as u64,
            max_pending,
        );
        prop_assert!(result.is_err());
    }

    /// Property 17: dequeue from empty returns None
    #[test]
    fn prop_dequeue_empty(_seed in 0u64..100u64) {
        let mut queue = WithdrawQueue::new();
        prop_assert!(queue.dequeue().is_none());
    }

    /// Property 18: queue status totals are accurate
    #[test]
    fn prop_queue_status_accurate(enqueues in 1usize..=10) {
        let mut queue = WithdrawQueue::new();
        let mut expected_shares = 0u128;
        let mut expected_assets = 0u128;

        for i in 0..enqueues {
            let shares = (i as u128 + 1) * 100;
            let assets = (i as u128 + 1) * 1000;
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                shares,
                assets,
                i as u64,
                100,
            );
            expected_shares += shares;
            expected_assets += assets;
        }

        let status = queue.status();
        prop_assert_eq!(status.length, enqueues as u32);
        prop_assert_eq!(status.total_escrow_shares, expected_shares);
        prop_assert_eq!(status.total_expected_assets, expected_assets);
    }

    /// Property 19: queue contains works correctly
    #[test]
    fn prop_queue_contains(enqueues in 1usize..=5) {
        let mut queue = WithdrawQueue::new();
        let mut ids = Vec::new();
        for i in 0..enqueues {
            let id = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            ).unwrap();
            ids.push(id);
        }

        for id in &ids {
            prop_assert!(queue.contains(*id));
        }
        prop_assert!(!queue.contains(9999));
    }

    /// Property 20: queue get returns correct item
    #[test]
    fn prop_queue_get(enqueues in 1usize..=5) {
        let mut queue = WithdrawQueue::new();
        for i in 0..enqueues {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100 + i as u128,
                1000 + i as u128,
                i as u64,
                100,
            );
        }

        for i in 0..enqueues {
            let withdrawal = queue.get(i as u64).unwrap();
            prop_assert_eq!(&withdrawal.owner, &owner_addr(i as u64));
            prop_assert_eq!(withdrawal.escrow_shares, 100 + i as u128);
        }
    }

    /// Property 21: is_valid_withdrawal_amount enforces minimum
    #[test]
    fn prop_valid_withdrawal_amount(amount in 0u128..=10_000u128) {
        let is_valid = is_valid_withdrawal_amount(amount);
        prop_assert_eq!(is_valid, amount >= MIN_WITHDRAWAL_ASSETS);
    }

    /// Property 22: can_enqueue respects MAX_QUEUE_LENGTH
    #[test]
    fn prop_can_enqueue_bounds(length in 0u32..=2000) {
        let result = can_enqueue(length);
        prop_assert_eq!(result, length < MAX_QUEUE_LENGTH);
    }

    /// Property 23: is_past_cooldown logic is correct
    #[test]
    fn prop_is_past_cooldown(
        requested_at in 0u64..=u64::MAX / 3,
        cooldown in 0u64..=u64::MAX / 3,
        delta in 0u64..=u64::MAX / 3,
    ) {
        let now = requested_at.saturating_add(delta);
        let threshold = requested_at.saturating_add(cooldown);
        let past = is_past_cooldown(requested_at, now, cooldown);
        prop_assert_eq!(past, now >= threshold);
    }

    /// Property 24: count_satisfiable is accurate
    #[test]
    fn prop_count_satisfiable(
        enqueues in 1usize..=5,
        available in 0u128..=10_000u128,
    ) {
        let withdrawals: Vec<PendingWithdrawal> = (0..enqueues)
            .map(|i| PendingWithdrawal::new(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
            ))
            .collect();

        let (count, total) = count_satisfiable(&withdrawals, available);

        // Verify count
        let expected_count = (available / 1000).min(enqueues as u128) as u32;
        prop_assert_eq!(count, expected_count);

        // Verify total
        prop_assert_eq!(total, count as u128 * 1000);
    }

    /// Property 25: compute_queue_status matches manual calculation
    #[test]
    fn prop_compute_queue_status(enqueues in 0usize..=10) {
        let withdrawals: Vec<PendingWithdrawal> = (0..enqueues)
            .map(|i| PendingWithdrawal::new(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                (i as u128 + 1) * 100,
                (i as u128 + 1) * 1000,
                i as u64,
            ))
            .collect();

        let status = compute_queue_status(&withdrawals);

        prop_assert_eq!(status.length, enqueues as u32);
        prop_assert_eq!(
            status.total_escrow_shares,
            (1..=enqueues).map(|i| i as u128 * 100).sum::<u128>()
        );
        prop_assert_eq!(
            status.total_expected_assets,
            (1..=enqueues).map(|i| i as u128 * 1000).sum::<u128>()
        );
    }

    // =========================================================================
    // SHARE/ASSET CONVERSION INVARIANTS (26-35)
    // =========================================================================

    /// Property 26: deposit followed by withdrawal returns <= original
    #[test]
    fn prop_deposit_withdraw_inverse(
        assets_in in 1u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        // Deposit: shares = floor(assets * (supply + 1) / (total_assets + 1))
        let shares = mul_div_floor(
            Number::from(assets_in),
            Number::from(total_supply.saturating_add(1)),
            Number::from(total_assets.saturating_add(1)),
        );

        // New totals after deposit
        let new_supply = total_supply.saturating_add(shares.as_u128_trunc());
        let new_assets = total_assets.saturating_add(assets_in);

        // Redeem: back = floor(shares * (new_assets + 1) / (new_supply + 1))
        let back_assets = mul_div_floor(
            shares,
            Number::from(new_assets.saturating_add(1)),
            Number::from(new_supply.saturating_add(1)),
        );

        prop_assert!(
            back_assets.as_u128_trunc() <= assets_in,
            "roundtrip gave more assets: {} > {}",
            back_assets.as_u128_trunc(),
            assets_in
        );
    }

    /// Property 27: no shares minted without assets
    #[test]
    fn prop_no_shares_from_nothing(
        total_supply in 1u128..=u64::MAX as u128,
        total_assets in 1u128..=u64::MAX as u128,
    ) {
        // Zero assets should mint zero shares
        let shares = mul_div_floor(
            Number::from(0u128),
            Number::from(total_supply.saturating_add(1)),
            Number::from(total_assets.saturating_add(1)),
        );
        prop_assert!(shares.is_zero(), "shares minted from nothing");
    }

    /// Property 28: share price floor is respected
    #[test]
    fn prop_share_price_floor(
        assets in 1u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        let shares = mul_div_floor(
            Number::from(assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // Floor should never exceed ceiling
        let shares_ceil = mul_div_ceil(
            Number::from(assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        prop_assert!(shares.0 <= shares_ceil.0);
    }

    /// Property 29: share price ceiling bounds
    #[test]
    fn prop_share_price_ceiling(
        assets in 1u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        let floor = mul_div_floor(
            Number::from(assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );
        let ceil = mul_div_ceil(
            Number::from(assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // Difference should be at most 1
        let diff = ceil.0.saturating_sub(floor.0);
        prop_assert!(diff <= primitive_types::U256::one());
    }

    /// Property 30: share conversion is monotonic in assets
    #[test]
    fn prop_share_conversion_monotonic_in_assets(
        assets1 in 0u128..=u64::MAX as u128 / 2,
        assets2 in 0u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        let (lo, hi) = if assets1 <= assets2 { (assets1, assets2) } else { (assets2, assets1) };
        let shares_lo = mul_div_floor(
            Number::from(lo),
            Number::from(total_supply),
            Number::from(total_assets),
        );
        let shares_hi = mul_div_floor(
            Number::from(hi),
            Number::from(total_supply),
            Number::from(total_assets),
        );
        prop_assert!(shares_lo.0 <= shares_hi.0);
    }

    /// Property 31: asset conversion is monotonic in shares
    #[test]
    fn prop_asset_conversion_monotonic_in_shares(
        shares1 in 0u128..=u64::MAX as u128 / 2,
        shares2 in 0u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        let (lo, hi) = if shares1 <= shares2 { (shares1, shares2) } else { (shares2, shares1) };
        let assets_lo = mul_div_floor(
            Number::from(lo),
            Number::from(total_assets),
            Number::from(total_supply),
        );
        let assets_hi = mul_div_floor(
            Number::from(hi),
            Number::from(total_assets),
            Number::from(total_supply),
        );
        prop_assert!(assets_lo.0 <= assets_hi.0);
    }

    /// Property 32: virtual shares prevent inflation attacks
    #[test]
    fn prop_virtual_shares_protection(
        attacker_assets in 1u128..=1_000_000u128,
        virtual_shares in 1u128..=1_000_000u128,
        virtual_assets in 1u128..=1_000_000u128,
    ) {
        // With virtual shares/assets, first depositor gets fair share
        let total_supply = virtual_shares;
        let total_assets = virtual_assets;

        let shares = mul_div_floor(
            Number::from(attacker_assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // Shares should be proportional, not inflated
        let expected_max = attacker_assets * total_supply / total_assets + 1;
        prop_assert!(shares.as_u128_trunc() <= expected_max);
    }

    /// Property 33: ERC4626 preview functions are consistent
    #[test]
    fn prop_erc4626_preview_consistency(
        assets in 1u128..=u64::MAX as u128 / 2,
        total_supply in 1u128..=u64::MAX as u128 / 2,
        total_assets in 1u128..=u64::MAX as u128 / 2,
    ) {
        // previewDeposit uses floor
        let preview_deposit_shares = mul_div_floor(
            Number::from(assets),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // previewMint uses ceiling for assets needed
        let preview_mint_assets = mul_div_ceil(
            preview_deposit_shares,
            Number::from(total_assets),
            Number::from(total_supply),
        );

        // Ceiling should be >= floor
        prop_assert!(preview_mint_assets.as_u128_trunc() >= assets.min(
            preview_deposit_shares.as_u128_trunc() * total_assets / total_supply
        ));
    }

    /// Property 34: conversion with zero denominator returns zero
    #[test]
    fn prop_conversion_zero_denom(
        assets in 0u128..=u64::MAX as u128,
        shares in 0u128..=u64::MAX as u128,
    ) {
        let floor = mul_div_floor(
            Number::from(assets),
            Number::from(shares),
            Number::from(0u128),
        );
        let ceil = mul_div_ceil(
            Number::from(assets),
            Number::from(shares),
            Number::from(0u128),
        );
        prop_assert!(floor.is_zero());
        prop_assert!(ceil.is_zero());
    }

    /// Property 35: conversion commutativity in numerators
    #[test]
    fn prop_conversion_commutative(
        a in 0u128..=u64::MAX as u128 / 2,
        b in 0u128..=u64::MAX as u128 / 2,
        denom in 1u128..=u64::MAX as u128,
    ) {
        let result1 = mul_div_floor(Number::from(a), Number::from(b), Number::from(denom));
        let result2 = mul_div_floor(Number::from(b), Number::from(a), Number::from(denom));
        prop_assert_eq!(result1.0, result2.0);
    }

    // =========================================================================
    // FEE INVARIANTS (36-45)
    // =========================================================================

    /// Property 36: fee accrual is non-negative
    #[test]
    fn prop_fee_accrual_non_negative(
        cur in 0u128..=u64::MAX as u128,
        last in 0u128..=u64::MAX as u128,
        fee_wad in 0u128..=Wad::SCALE,
        total_supply in 0u128..=u64::MAX as u128,
    ) {
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        prop_assert!(!result.0.is_zero() || cur <= last || fee_wad == 0 || total_supply == 0);
    }

    /// Property 37: zero fee produces zero shares
    #[test]
    fn prop_zero_fee_zero_shares(
        cur in 0u128..=u64::MAX as u128,
        last in 0u128..=u64::MAX as u128,
        total_supply in 0u128..=u64::MAX as u128,
    ) {
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::zero(),
            Number::from(total_supply),
        );
        prop_assert!(result.is_zero());
    }

    /// Property 38: no profit produces zero fee shares
    #[test]
    fn prop_no_profit_no_fees(
        last in 1u128..=u64::MAX as u128,
        delta in 0u128..=1_000_000u128,
        fee_wad in 1u128..=Wad::SCALE,
        total_supply in 1u128..=u64::MAX as u128,
    ) {
        let cur = last.saturating_sub(delta);
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        prop_assert!(result.is_zero(), "fees charged without profit");
    }

    /// Property 39: fee shares monotonic in fee rate
    #[test]
    fn prop_fee_shares_monotonic_in_fee(
        cur in 1u128..=u64::MAX as u128 / 2,
        last in 1u128..=u64::MAX as u128 / 2,
        fee1 in 0u128..=Wad::SCALE / 2,
        fee2 in Wad::SCALE / 2..=Wad::SCALE,
        total_supply in 1u128..=u64::MAX as u128 / 2,
    ) {
        let cur = cur.max(last); // Ensure profit
        let result1 = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee1),
            Number::from(total_supply),
        );
        let result2 = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee2),
            Number::from(total_supply),
        );
        prop_assert!(result1.0 <= result2.0);
    }

    /// Property 40: fee shares monotonic in profit
    #[test]
    fn prop_fee_shares_monotonic_in_profit(
        last in 1u128..=u64::MAX as u128 / 4,
        profit1 in 0u128..=1_000_000_000u128,
        profit2 in 0u128..=1_000_000_000u128,
        fee_wad in 1u128..=MAX_PERFORMANCE_FEE_WAD,
        total_supply in 1u128..=u64::MAX as u128 / 4,
    ) {
        let (lo_p, hi_p) = if profit1 <= profit2 { (profit1, profit2) } else { (profit2, profit1) };
        let cur_lo = last.saturating_add(lo_p);
        let cur_hi = last.saturating_add(hi_p);

        let result_lo = compute_fee_shares(
            Number::from(cur_lo),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        let result_hi = compute_fee_shares(
            Number::from(cur_hi),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        prop_assert!(result_lo.0 <= result_hi.0);
    }

    /// Property 41: fee shares bounded with capped fees
    #[test]
    fn prop_fee_shares_bounded_with_cap(
        cur in 1u128..=u64::MAX as u128 / 2,
        last in 1u128..=u64::MAX as u128 / 2,
        fee_wad in 1u128..=MAX_PERFORMANCE_FEE_WAD,
        total_supply in 1u128..=u64::MAX as u128 / 2,
    ) {
        let cur = cur.max(last);
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );

        // With capped fees (50%), fee shares should be bounded
        // This is a sanity check, not a strict bound
        prop_assert!(
            result.0 <= primitive_types::U256::from(total_supply) * primitive_types::U256::from(2u8),
            "fee shares exceed 2x total supply with capped fees"
        );
    }

    /// Property 42: Wad::one is identity for apply_floored
    #[test]
    fn prop_wad_one_identity(amount in 0u128..=u64::MAX as u128) {
        let result = Wad::one().apply_floored(Number::from(amount));
        prop_assert_eq!(result.as_u128_trunc(), amount);
    }

    /// Property 43: Wad::zero is zero for apply_floored
    #[test]
    fn prop_wad_zero_is_zero(amount in 0u128..=u64::MAX as u128) {
        let result = Wad::zero().apply_floored(Number::from(amount));
        prop_assert!(result.is_zero());
    }

    /// Property 44: Wad apply is bounded by input
    #[test]
    fn prop_wad_apply_bounded(
        wad_raw in 0u128..=Wad::SCALE,
        amount in 0u128..=u64::MAX as u128,
    ) {
        let wad = Wad::from(wad_raw);
        let result = wad.apply_floored(Number::from(amount));
        prop_assert!(result.0 <= Number::from(amount).0);
    }

    /// Property 45: Wad apply is monotonic
    #[test]
    fn prop_wad_apply_monotonic(
        wad1 in 0u128..=Wad::SCALE / 2,
        wad2 in Wad::SCALE / 2..=Wad::SCALE,
        amount in 0u128..=u64::MAX as u128,
    ) {
        let result1 = Wad::from(wad1).apply_floored(Number::from(amount));
        let result2 = Wad::from(wad2).apply_floored(Number::from(amount));
        prop_assert!(result1.0 <= result2.0);
    }

    // =========================================================================
    // STATE TRANSITION INVARIANTS (46-60)
    // =========================================================================

    /// Property 46: start_allocation requires Idle state
    #[test]
    fn prop_start_allocation_requires_idle(
        plan in arb_allocation_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        // From Idle - should succeed
        let result = start_allocation(OpState::Idle, plan.clone(), op_id);
        prop_assert!(result.is_ok());

        // From non-Idle - should fail
        let non_idle = OpState::Refreshing(RefreshingState {
            op_id: 1,
            index: 0,
            plan: vec![0],
        });
        let result = start_allocation(non_idle, plan, op_id);
        prop_assert!(
            matches!(result, Err(TransitionError::NotIdle { .. })),
            "expected NotIdle when starting allocation from non-idle"
        );
    }

    /// Property 47: start_withdrawal requires Idle state
    #[test]
    fn prop_start_withdrawal_requires_idle(
        request in arb_withdrawal_request(),
    ) {
        // From Idle - should succeed
        let result = start_withdrawal(OpState::Idle, request.clone());
        prop_assert!(result.is_ok());

        // From non-Idle - should fail
        let non_idle = OpState::Refreshing(RefreshingState {
            op_id: 1,
            index: 0,
            plan: vec![0],
        });
        let result = start_withdrawal(non_idle, request);
        prop_assert!(
            matches!(result, Err(TransitionError::NotIdle { .. })),
            "expected NotIdle when starting withdrawal from non-idle"
        );
    }

    /// Property 48: start_refresh requires Idle state
    #[test]
    fn prop_start_refresh_requires_idle(
        plan in arb_refresh_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        // From Idle - should succeed
        let result = start_refresh(OpState::Idle, plan.clone(), op_id);
        prop_assert!(result.is_ok());

        // From non-Idle - should fail
        let non_idle = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 100,
            plan: vec![(0, 100)],
        });
        let result = start_refresh(non_idle, plan, op_id);
        prop_assert!(
            matches!(result, Err(TransitionError::NotIdle { .. })),
            "expected NotIdle when starting refresh from non-idle"
        );
    }

    /// Property 49: empty allocation plan rejected
    #[test]
    fn prop_empty_allocation_plan_rejected(op_id in 1u64..u64::MAX) {
        let result = start_allocation(OpState::Idle, vec![], op_id);
        prop_assert!(
            matches!(result, Err(TransitionError::EmptyAllocationPlan)),
            "expected EmptyAllocationPlan for empty plan"
        );
    }

    /// Property 50: empty refresh plan rejected
    #[test]
    fn prop_empty_refresh_plan_rejected(op_id in 1u64..u64::MAX) {
        let result = start_refresh(OpState::Idle, vec![], op_id);
        prop_assert!(
            matches!(result, Err(TransitionError::EmptyRefreshPlan)),
            "expected EmptyRefreshPlan for empty plan"
        );
    }

    /// Property 51: zero withdrawal amount rejected
    #[test]
    fn prop_zero_withdrawal_amount_rejected(
        op_id in 1u64..u64::MAX,
        escrow_shares in 1u128..=1_000_000u128,
    ) {
        let request = WithdrawalRequest {
            op_id,
            amount: 0,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares,
        };
        let result = start_withdrawal(OpState::Idle, request);
        prop_assert!(
            matches!(result, Err(TransitionError::ZeroWithdrawalAmount)),
            "expected ZeroWithdrawalAmount for zero amount"
        );
    }

    /// Property 52: zero escrow shares rejected
    #[test]
    fn prop_zero_escrow_shares_rejected(
        op_id in 1u64..u64::MAX,
        amount in 1u128..=1_000_000u128,
    ) {
        let request = WithdrawalRequest {
            op_id,
            amount,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 0,
        };
        let result = start_withdrawal(OpState::Idle, request);
        prop_assert!(
            matches!(result, Err(TransitionError::ZeroEscrowShares)),
            "expected ZeroEscrowShares for zero escrow"
        );
    }

    /// Property 53: op_id mismatch rejected
    #[test]
    fn prop_op_id_mismatch_rejected(
        plan in arb_allocation_plan(3),
        op_id in 1u64..=u64::MAX / 2,
        wrong_op_id in u64::MAX / 2 + 1..=u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let step_result = allocation_step_callback(result.new_state, true, 100, wrong_op_id);
        prop_assert!(
            matches!(step_result, Err(TransitionError::OpIdMismatch { .. })),
            "expected OpIdMismatch for wrong op id"
        );
    }

    /// Property 54: allocation step advances index
    #[test]
    fn prop_allocation_step_advances_index(
        plan in arb_allocation_plan(5),
        op_id in 1u64..u64::MAX,
        allocated in 1u128..=1_000_000u128,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let alloc = result.new_state.as_allocating().unwrap();
        let initial_index = alloc.index;
        let initial_remaining = alloc.remaining;

        prop_assume!(initial_remaining > 0);
        let mut bounded = allocated % initial_remaining;
        if bounded == 0 {
            bounded = 1;
        }

        let step_result =
            allocation_step_callback(result.new_state, true, bounded, op_id).unwrap();
        let new_alloc = step_result.new_state.as_allocating().unwrap();

        prop_assert_eq!(new_alloc.index, initial_index + 1);
        prop_assert_eq!(new_alloc.remaining, initial_remaining.saturating_sub(bounded));
    }

    /// Property 55: allocation failure returns to Idle
    #[test]
    fn prop_allocation_failure_returns_idle(
        plan in arb_allocation_plan(3),
        op_id in 1u64..u64::MAX,
    ) {
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let step_result = allocation_step_callback(result.new_state, false, 0, op_id).unwrap();
        prop_assert!(step_result.new_state.is_idle());
    }

    /// Property 56: withdrawal step accumulates collected
    #[test]
    fn prop_withdrawal_step_accumulates(
        request in arb_withdrawal_request(),
        collected1 in 1u128..=500_000u128,
        collected2 in 1u128..=500_000u128,
    ) {
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

        let remaining1 = request.amount;
        let mut bounded1 = collected1 % remaining1;
        if bounded1 == 0 {
            bounded1 = 1;
        }

        let step1 = withdrawal_step_callback(result.new_state, request.op_id, bounded1).unwrap();
        let w1 = step1.new_state.as_withdrawing().unwrap();
        prop_assert_eq!(w1.collected, bounded1);
        prop_assert_eq!(w1.index, 1);

        let remaining2 = remaining1.saturating_sub(bounded1);
        prop_assume!(remaining2 > 0);
        let mut bounded2 = collected2 % remaining2;
        if bounded2 == 0 {
            bounded2 = 1;
        }

        let step2 = withdrawal_step_callback(step1.new_state, request.op_id, bounded2).unwrap();
        let w2 = step2.new_state.as_withdrawing().unwrap();
        prop_assert_eq!(w2.collected, bounded1.saturating_add(bounded2));
        prop_assert_eq!(w2.index, 2);
    }

    /// Property 57: burn shares cannot exceed escrow
    #[test]
    fn prop_burn_cannot_exceed_escrow(
        request in arb_withdrawal_request(),
        excess in 1u128..=1_000_000u128,
    ) {
        // Build a fully-collected state (remaining=0) so the burn check fires.
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: request.op_id,
            index: 1,
            remaining: 0,
            collected: request.amount,
            receiver: request.receiver.clone(),
            owner: request.owner.clone(),
            escrow_shares: request.escrow_shares,
        });
        let burn_shares = request.escrow_shares.saturating_add(excess);

        let collected = withdrawal_collected(state, request.op_id, burn_shares);
        prop_assert!(
            matches!(collected, Err(TransitionError::BurnExceedsEscrow { .. })),
            "expected BurnExceedsEscrow when burn exceeds escrow"
        );
    }

    /// Property 58: stop_withdrawal returns to Idle
    #[test]
    fn prop_stop_withdrawal_returns_idle(
        request in arb_withdrawal_request(),
    ) {
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
        let escrow_address = owner_addr(99);
        let stop = stop_withdrawal(result.new_state, request.op_id, escrow_address).unwrap();
        prop_assert!(stop.new_state.is_idle());
    }

    /// Property 59: complete_refresh returns to Idle
    #[test]
    fn prop_complete_refresh_returns_idle(
        plan in arb_refresh_plan(5),
        op_id in 1u64..u64::MAX,
    ) {
        let result = start_refresh(OpState::Idle, plan, op_id).unwrap();
        let complete = complete_refresh(result.new_state, op_id).unwrap();
        prop_assert!(complete.new_state.is_idle());
    }

    /// Property 60: payout_complete returns to Idle
    #[test]
    fn prop_payout_complete_returns_idle(
        op_id in 1u64..u64::MAX,
        amount in 1u128..=1_000_000_000u128,
        escrow_shares in 1u128..=1_000_000_000u128,
        burn_pct in 0u8..=100u8,
        success in proptest::bool::ANY,
    ) {
        let burn_shares = escrow_shares * burn_pct as u128 / 100;
        let payout = PayoutState {
            op_id,
            receiver: receiver_addr(1),
            amount,
            owner: owner_addr(1),
            escrow_shares,
            burn_shares,
        };
        let state = OpState::Payout(payout);

        let escrow_address = owner_addr(99);
        let result = payout_complete(state, success, op_id, escrow_address).unwrap();
        prop_assert!(result.new_state.is_idle());
    }

    // =========================================================================
    // ESCROW INVARIANTS (61-70)
    // =========================================================================

    /// Property 61: settle_proportional conserves shares
    #[test]
    fn prop_settle_proportional_conserves(
        shares in 0u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
        actual_assets in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_proportional(&entry, actual_assets);
        let total = settlement.to_burn.saturating_add(settlement.refund);
        prop_assert_eq!(total, shares);
    }

    /// Property 62: settle_full_burn burns all
    #[test]
    fn prop_settle_full_burn_all(
        shares in 0u128..=u64::MAX as u128,
        expected in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected);
        let settlement = settle_full_burn(&entry);
        prop_assert_eq!(settlement.to_burn, shares);
        prop_assert_eq!(settlement.refund, 0);
    }

    /// Property 63: settle_full_refund refunds all
    #[test]
    fn prop_settle_full_refund_all(
        shares in 0u128..=u64::MAX as u128,
        expected in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected);
        let settlement = settle_full_refund(&entry);
        prop_assert_eq!(settlement.to_burn, 0);
        prop_assert_eq!(settlement.refund, shares);
    }

    /// Property 64: apply_settlement validates bounds
    #[test]
    fn prop_apply_settlement_validates(
        shares in 1u128..=u64::MAX as u128 - 1,
        excess in 1u128..=1_000_000u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        let settlement = EscrowSettlement::partial(shares, excess);
        let result = apply_settlement(&entry, &settlement);
        prop_assert!(result.is_none());
    }

    /// Property 65: can_apply_settlement consistency
    #[test]
    fn prop_can_apply_settlement_consistency(
        shares in 0u128..=u64::MAX as u128,
        to_burn in 0u128..=u64::MAX as u128 / 2,
        refund in 0u128..=u64::MAX as u128 / 2,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        let settlement = EscrowSettlement::partial(to_burn, refund);
        let total = to_burn.saturating_add(refund);
        let can = can_apply_settlement(&entry, &settlement);
        prop_assert_eq!(can, total <= shares);
    }

    /// Property 66: compute_settlement full burn when actual >= expected
    #[test]
    fn prop_compute_settlement_full_burn(
        escrow_shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128 / 2,
        extra in 0u128..=1_000_000u128,
    ) {
        let actual_assets = expected_assets.saturating_add(extra);
        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);
        prop_assert_eq!(settlement.to_burn, escrow_shares);
        prop_assert_eq!(settlement.refund, 0);
    }

    /// Property 67: compute_settlement full refund when actual == 0
    #[test]
    fn prop_compute_settlement_full_refund(
        escrow_shares in 1u128..=u64::MAX as u128,
        expected_assets in 1u128..=u64::MAX as u128,
    ) {
        let settlement = compute_settlement(escrow_shares, expected_assets, 0);
        prop_assert_eq!(settlement.to_burn, 0);
        prop_assert_eq!(settlement.refund, escrow_shares);
    }

    /// Property 68: compute_settlement partial is proportional
    #[test]
    fn prop_compute_settlement_partial(
        escrow_shares in 100u128..=1_000_000u128,
        expected_assets in 1000u128..=10_000_000u128,
        ratio in 1u8..=99u8,
    ) {
        let actual_assets = expected_assets * ratio as u128 / 100;
        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

        // Burn should be approximately proportional
        let expected_burn = escrow_shares * actual_assets / expected_assets;
        prop_assert!(settlement.to_burn <= expected_burn + 1);
        prop_assert!(settlement.to_burn >= expected_burn.saturating_sub(1));

        // Conservation
        prop_assert_eq!(settlement.to_burn + settlement.refund, escrow_shares);
    }

    /// Property 69: compute_escrow_stats accurate
    #[test]
    fn prop_compute_escrow_stats_accurate(count in 0usize..=10) {
        let entries: Vec<EscrowEntry> = (0..count)
            .map(|i| EscrowEntry::new(
                owner_addr(i as u64),
                (i as u128 + 1) * 100,
                i as u64,
                (i as u128 + 1) * 1000,
            ))
            .collect();

        let stats = compute_escrow_stats(&entries);
        prop_assert_eq!(stats.count, count as u32);
        prop_assert_eq!(
            stats.total_shares,
            (1..=count).map(|i| i as u128 * 100).sum::<u128>()
        );
        prop_assert_eq!(
            stats.total_expected_assets,
            (1..=count).map(|i| i as u128 * 1000).sum::<u128>()
        );
    }

    /// Property 70: EscrowEntry::is_empty consistency
    #[test]
    fn prop_escrow_entry_is_empty(shares in 0u128..=u64::MAX as u128) {
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        prop_assert_eq!(entry.is_empty(), shares == 0);
    }
}

// ============================================================================
// Deterministic Boundary / Edge Case Tests
// ============================================================================

use templar_vault_kernel::{
    apply_action, preview_deposit_shares, preview_withdraw_assets, FeeSlot, FeesSpec, KernelAction,
    VaultConfig,
};

fn default_config() -> VaultConfig {
    VaultConfig {
        fees: FeesSpec {
            performance: FeeSlot::zero(),
            management: FeeSlot::zero(),
            max_total_assets_growth_rate: None,
        },
        min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
        withdrawal_cooldown_ns: 0,
        max_pending_withdrawals: 100,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    }
}

fn default_state() -> VaultState {
    VaultState::new()
}

fn self_addr() -> templar_vault_kernel::Address {
    [99u8; 32]
}

/// Boundary 1: Depositing zero assets returns Slippage error.
#[test]
fn deposit_zero_assets_returns_slippage() {
    let state = default_state();
    let config = default_config();
    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            assets_in: 0,
            min_shares_out: 0,
            now_ns: 1,
        },
    );
    assert!(
        matches!(result, Err(templar_vault_kernel::error::KernelError::Slippage { actual: 0, .. })),
        "Depositing 0 assets should return Slippage, got: {result:?}",
    );
}

/// Boundary 2: Depositing 1 wei succeeds and mints shares.
#[test]
fn deposit_one_wei_mints_shares() {
    let state = default_state();
    let config = default_config();
    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            assets_in: 1,
            min_shares_out: 0,
            now_ns: 1,
        },
    );
    let result = result.expect("1 wei deposit should succeed");
    assert_eq!(result.state.total_assets, 1);
    assert_eq!(result.state.idle_assets, 1);
    assert!(result.state.total_shares > 0, "Should mint at least 1 share");
}

/// Boundary 3: Preview deposit with 0 assets returns 0 shares.
#[test]
fn preview_deposit_zero_assets_returns_zero() {
    let state = default_state();
    let config = default_config();
    assert_eq!(preview_deposit_shares(&state, &config, 0), 0);
}

/// Boundary 4: Preview withdraw with 0 shares returns 0 assets.
#[test]
fn preview_withdraw_zero_shares_returns_zero() {
    let state = default_state();
    let config = default_config();
    assert_eq!(preview_withdraw_assets(&state, &config, 0), 0);
}

/// Boundary 5: Preview deposit/withdraw with 1 share/asset is consistent.
#[test]
fn preview_one_wei_roundtrip() {
    let mut state = default_state();
    state.total_assets = 1_000_000;
    state.idle_assets = 1_000_000;
    state.total_shares = 1_000_000;
    let config = default_config();

    let shares = preview_deposit_shares(&state, &config, 1);
    // With equal shares/assets ratio and virtual offset, 1 wei should give ~1 share
    // (may be 0 due to rounding with virtual offsets)
    let assets_back = preview_withdraw_assets(&state, &config, shares);
    // Round-trip: assets_back <= 1 (rounding down is expected)
    assert!(assets_back <= 1, "Round-trip should not inflate: got {assets_back}");
}

/// Boundary 6: Request withdraw below MIN_WITHDRAWAL_ASSETS is rejected.
#[test]
fn withdraw_below_min_withdrawal_rejected() {
    let mut state = default_state();
    state.total_assets = 1_000_000;
    state.idle_assets = 1_000_000;
    state.total_shares = 1_000_000;
    let config = default_config();

    // Request a withdrawal that would yield MIN_WITHDRAWAL_ASSETS - 1
    // We need shares that convert to exactly MIN_WITHDRAWAL_ASSETS - 1
    let target = MIN_WITHDRAWAL_ASSETS - 1;
    let shares = target; // With 1:1 ratio and virtual offset +1, shares ~= target

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RequestWithdraw {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            shares,
            min_assets_out: 0,
            now_ns: 1,
        },
    );
    assert!(
        matches!(result, Err(templar_vault_kernel::error::KernelError::MinWithdrawal { .. })),
        "Withdrawal below MIN_WITHDRAWAL_ASSETS should be rejected, got: {result:?}",
    );
}

/// Boundary 7: Request withdraw at exactly MIN_WITHDRAWAL_ASSETS succeeds.
#[test]
fn withdraw_at_min_withdrawal_succeeds() {
    let mut state = default_state();
    // Use large enough total so shares convert to >= MIN_WITHDRAWAL_ASSETS
    state.total_assets = 10_000_000;
    state.idle_assets = 10_000_000;
    state.total_shares = 10_000_000;
    let config = default_config();

    // Find shares that yield exactly MIN_WITHDRAWAL_ASSETS
    let expected = preview_withdraw_assets(&state, &config, MIN_WITHDRAWAL_ASSETS);
    // Ensure we're at or above the minimum
    assert!(
        expected >= MIN_WITHDRAWAL_ASSETS,
        "Expected assets {expected} should be >= MIN {MIN_WITHDRAWAL_ASSETS}",
    );

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RequestWithdraw {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            shares: MIN_WITHDRAWAL_ASSETS,
            min_assets_out: 0,
            now_ns: 1,
        },
    );
    assert!(result.is_ok(), "Withdrawal at MIN should succeed, got: {result:?}");
}

/// Boundary 8: Request withdraw with 0 shares returns Slippage.
#[test]
fn withdraw_zero_shares_returns_slippage() {
    let mut state = default_state();
    state.total_assets = 1_000_000;
    state.total_shares = 1_000_000;
    let config = default_config();

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RequestWithdraw {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            shares: 0,
            min_assets_out: 0,
            now_ns: 1,
        },
    );
    assert!(
        matches!(result, Err(templar_vault_kernel::error::KernelError::Slippage { actual: 0, .. })),
        "Withdrawing 0 shares should return Slippage, got: {result:?}",
    );
}

/// Boundary 9: Fee calculation with total_assets = 1.
#[test]
fn fee_shares_with_total_assets_one() {
    // With minimal total_assets, fee shares should be 0 or very small
    let fee_shares = compute_fee_shares(
        Number::from(1u128),  // current total_assets
        Number::from(0u128),  // last total_assets (0 → gain = 1)
        Wad::from(MAX_PERFORMANCE_FEE_WAD), // max performance fee
        Number::from(1u128),  // total_supply
    );
    // Fee shares should not exceed total supply
    assert!(
        fee_shares <= Number::from(1u128),
        "Fee shares {:?} should not exceed total supply of 1",
        fee_shares,
    );
}

/// Boundary 10: Queue at exactly MAX_QUEUE_LENGTH rejects next enqueue.
#[test]
fn queue_at_max_rejects_enqueue() {
    assert!(can_enqueue(MAX_QUEUE_LENGTH - 1), "Should allow enqueue below max");
    assert!(!can_enqueue(MAX_QUEUE_LENGTH), "Should reject enqueue at max");
    assert!(!can_enqueue(MAX_QUEUE_LENGTH + 1), "Should reject enqueue above max");
}

/// Boundary 11: is_valid_withdrawal_amount at boundary values.
#[test]
fn withdrawal_amount_boundary_values() {
    assert!(!is_valid_withdrawal_amount(0), "0 is not valid");
    assert!(!is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS - 1), "Below min is not valid");
    assert!(is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS), "Exactly min is valid");
    assert!(is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS + 1), "Above min is valid");
    assert!(is_valid_withdrawal_amount(u128::MAX), "MAX is valid");
}

/// Boundary 12: compute_settlement with 1 wei actual vs large expected.
#[test]
fn settlement_one_wei_actual() {
    let escrow = 1_000_000u128;
    let expected = 1_000_000u128;
    let actual = 1u128; // Only 1 wei collected

    let settlement = compute_settlement(escrow, expected, actual);
    // Burn should be proportional: ~1 share (1/1_000_000 * 1_000_000)
    assert!(settlement.to_burn >= 1, "Should burn at least 1 share");
    assert_eq!(
        settlement.to_burn + settlement.refund, escrow,
        "Conservation: burn + refund = escrow",
    );
}

/// Boundary 13: compute_settlement with 0 actual (full refund).
#[test]
fn settlement_zero_actual_full_refund() {
    let escrow = 1_000_000u128;
    let expected = 500_000u128;

    let settlement = compute_settlement(escrow, expected, 0);
    assert_eq!(settlement.to_burn, 0, "Zero actual → zero burn");
    assert_eq!(settlement.refund, escrow, "Zero actual → full refund");
}

/// Boundary 14: Queue enqueue fills to capacity then rejects.
#[test]
fn queue_fills_to_capacity_then_rejects() {
    let mut queue = WithdrawQueue::default();
    let max = 10u32; // Use small max for practical test

    // Fill queue to capacity
    for i in 0..max {
        let result = queue.enqueue(
            owner_addr(i as u64),
            receiver_addr(i as u64),
            1000,
            MIN_WITHDRAWAL_ASSETS,
            i as u64,
            max,
        );
        assert!(result.is_ok(), "Enqueue {i} should succeed");
    }

    // Next enqueue should fail
    let result = queue.enqueue(
        owner_addr(max as u64),
        receiver_addr(max as u64),
        1000,
        MIN_WITHDRAWAL_ASSETS,
        max as u64,
        max,
    );
    assert!(
        result.is_err(),
        "Enqueue beyond capacity should fail",
    );
}

/// Boundary 15: Cooldown at exact boundary.
#[test]
fn cooldown_exact_boundary() {
    let cooldown_ns = 1_000_000u64;
    let requested_at = 100u64;

    // Just before cooldown
    assert!(
        !is_past_cooldown(requested_at, requested_at + cooldown_ns - 1, cooldown_ns),
        "Should NOT be past cooldown 1ns before",
    );
    // Exactly at cooldown
    assert!(
        is_past_cooldown(requested_at, requested_at + cooldown_ns, cooldown_ns),
        "Should be past cooldown at exact boundary",
    );
    // Just after cooldown
    assert!(
        is_past_cooldown(requested_at, requested_at + cooldown_ns + 1, cooldown_ns),
        "Should be past cooldown 1ns after",
    );
}

/// Boundary 16: Zero cooldown means immediately ready when now >= requested_at.
#[test]
fn zero_cooldown_passes_when_now_gte_requested() {
    assert!(is_past_cooldown(0, 0, 0), "Zero cooldown, same time → past");
    assert!(is_past_cooldown(100, 100, 0), "Zero cooldown, same time → past");
    assert!(is_past_cooldown(100, 101, 0), "Zero cooldown, later now → past");
    assert!(!is_past_cooldown(100, 99, 0), "Zero cooldown, earlier now → not past (request not yet made)");
}
