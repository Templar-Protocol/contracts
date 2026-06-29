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
use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};
use templar_vault_kernel::{
    apply_action,
    effects::{KernelEffect, KernelEvent},
    fee::FeeSlot,
    math::{
        number::Number,
        wad::{
            compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor, Wad,
            MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
        },
    },
    state::{
        escrow::{
            apply_settlement, can_apply_settlement, compute_escrow_stats, settle_proportional,
            EscrowEntry,
        },
        op_state::{
            AllocatingState, AllocationPlanEntry, OpState, PayoutState, RefreshingState,
            WithdrawingState,
        },
        queue::{
            can_enqueue, compute_queue_status, compute_settlement, count_satisfiable,
            is_past_cooldown, is_valid_withdrawal_amount, PendingWithdrawal, WithdrawQueue,
            MAX_QUEUE_LENGTH, MIN_WITHDRAWAL_ASSETS,
        },
        vault::{FeeAccrualAnchor, VaultState, MAX_PENDING},
    },
    transitions::{
        allocation_step_callback, complete_allocation, complete_refresh, payout_complete,
        start_allocation, start_refresh, start_withdrawal, stop_withdrawal, withdrawal_collected,
        withdrawal_step_callback, TransitionError, WithdrawalRequest,
    },
    types::EscrowSettlement,
    Address, FeesSpec, KernelAction, TimestampNs,
};

// Arbitrary Strategies

/// Generate a valid allocation plan
fn alloc_step(target_id: u32, amount: u128) -> AllocationPlanEntry {
    AllocationPlanEntry::new(target_id, amount)
}

fn arb_allocation_plan(max_len: usize) -> impl Strategy<Value = Vec<AllocationPlanEntry>> {
    proptest::collection::vec((0u32..100u32, 1u128..=1_000_000_000u128), 1..=max_len).prop_map(
        |steps| {
            steps
                .into_iter()
                .map(|(target_id, amount)| alloc_step(target_id, amount))
                .collect()
        },
    )
}

/// Generate a refresh plan (list of target IDs)
fn arb_refresh_plan(max_len: usize) -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::vec(0u32..100u32, 1..=max_len)
}

/// Generate a withdrawal request
fn arb_withdrawal_request() -> impl Strategy<Value = WithdrawalRequest> {
    (
        1u64..u64::MAX, // op_id
        1u64..u64::MAX,
        1u128..=1_000_000_000u128, // amount
        1u128..=1_000_000_000u128, // escrow_shares
    )
        .prop_map(
            |(op_id, request_id, amount, escrow_shares)| WithdrawalRequest {
                op_id,
                request_id,
                amount,
                receiver: receiver_addr(op_id),
                owner: owner_addr(op_id),
                escrow_shares,
            },
        )
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
                TimestampNs(requested_at_ns),
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
        .prop_map(|(shares, ts, expected)| {
            EscrowEntry::new(owner_addr(1), shares, TimestampNs(ts), expected)
        })
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
            VaultState::with_initial(total, shares, idle, external, TimestampNs(ts))
        })
}

proptest! {
    /// Property 1: total_assets = idle_assets + external_assets
    /// Invariant: The fundamental accounting equation always holds.
    #[test]
    fn prop_total_assets_accounting(
        idle in 0u128..=u64::MAX as u128 / 2,
        external in 0u128..=u64::MAX as u128 / 2,
    ) {
        let total = idle.saturating_add(external);
        let state = VaultState::with_initial(total, 0, idle, external, TimestampNs(0));
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
        state.fee_anchor = FeeAccrualAnchor::new(total, TimestampNs(0));
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
        let state = VaultState::with_initial(total, shares, idle, external, TimestampNs(ts));
        prop_assert!(state.check_invariant());
        prop_assert_eq!(state.total_assets, total);
        prop_assert_eq!(state.total_shares, shares);
        prop_assert_eq!(state.fee_anchor.total_assets, total);
        prop_assert_eq!(state.fee_anchor.timestamp_ns, TimestampNs(ts));
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

    /// Property 6: op_id allocation panics on overflow
    #[test]
    fn prop_op_id_overflow_panics(_seed in 0u64..100u64) {
        let mut state = VaultState::new();
        state.next_op_id = u64::MAX;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = state.allocate_op_id();
        }));
        prop_assert!(panic.is_err());
    }

    /// Property 7: fee anchor update preserves structure
    #[test]
    fn prop_fee_anchor_update(
        old_assets in 0u128..=u64::MAX as u128,
        old_ts in 0u64..u64::MAX / 2,
        new_assets in 0u128..=u64::MAX as u128,
        new_ts in u64::MAX / 2..u64::MAX,
    ) {
        let mut anchor = FeeAccrualAnchor::new(old_assets, TimestampNs(old_ts));
        anchor.update(new_assets, TimestampNs(new_ts));
        prop_assert_eq!(anchor.total_assets, new_assets);
        prop_assert_eq!(anchor.timestamp_ns, TimestampNs(new_ts));
    }

    /// Property 8: zero fee anchor is valid
    #[test]
    fn prop_fee_anchor_zero(_seed in 0u64..100u64) {
        let anchor = FeeAccrualAnchor::zero();
        prop_assert_eq!(anchor.total_assets, 0);
        prop_assert_eq!(anchor.timestamp_ns, TimestampNs(0));
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
                TimestampNs(i as u64 * 1_000_000_000),
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
                100,
            );
        }
        prop_assert!(!queue.is_empty());
        prop_assert!(queue.pending_withdrawals().contains_key(&queue.next_withdraw_to_execute));
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
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
            TimestampNs(max_pending as u64),
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
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

    /// Property 22: can_enqueue respects MAX_QUEUE_LENGTH (alias of MAX_PENDING)
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
        let past = is_past_cooldown(TimestampNs(requested_at), TimestampNs(now), cooldown);
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
                TimestampNs(i as u64),
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
                TimestampNs(i as u64),
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

    /// Property 45a / A-031: fee minting near u128::MAX either succeeds or errors, never truncates.
    ///
    /// Fee minting now checks the full-width quotient against remaining supply before
    /// converting to `u128`.
    #[test]
    fn prop_a031_fee_mint_overflow_handled(
        total_supply in (u128::MAX - 1_000_000u128)..=u128::MAX,
        cur_total_assets in 2u128..=1_000_000u128,
        fee_wad in (Wad::SCALE / 10)..=Wad::SCALE,
    ) {
        let state = VaultState {
            total_assets: cur_total_assets,
            total_shares: total_supply,
            idle_assets: cur_total_assets,
            fee_anchor: FeeAccrualAnchor::new(1, TimestampNs(0)),
            ..VaultState::default()
        };

        let mut config = default_config();
        let performance = FeeSlot::new(Wad::from(fee_wad), Address([9u8; 32]));
        let management = FeeSlot::zero();
        config.fees = FeesSpec::new(performance, management, None);

        let result = apply_action(
            state,
            &config,
            None,
            &Address([0u8; 32]),
            KernelAction::RefreshFees {
                now_ns: TimestampNs(1),
            },
        );

        let fee_assets =
            Wad::from(fee_wad).apply_floored(Number::from(cur_total_assets - 1));
        let fee_shares = compute_fee_shares_from_assets(
            fee_assets,
            Number::from(cur_total_assets),
            Number::from(total_supply),
        );
        let fee_shares_trunc = fee_shares.as_u128_trunc();
        let fee_shares_sat = fee_shares.as_u128_saturating();
        let would_overflow =
            fee_shares_sat != fee_shares_trunc
                || total_supply.checked_add(fee_shares_trunc).is_none();

        match result {
            Ok(result) => {
                prop_assert!(!would_overflow);
                let minted: u128 = result
                    .effects
                    .iter()
                    .filter_map(|effect| match effect {
                        KernelEffect::MintShares { owner, shares }
                            if *owner == Address([9u8; 32]) =>
                        {
                            Some(*shares)
                        }
                        _ => None,
                    })
                    .sum();
                prop_assert_eq!(result.state.total_shares, total_supply + minted);
            }
            Err(_) => {
                prop_assert!(would_overflow);
            }
        }
    }

    /// Property 45b / A-031: nonzero deposits must not wrap to zero shares.
    ///
    /// At the Soroban `i128::MAX` share boundary with one effective asset, a two-asset
    /// deposit has a raw share quotient of exactly `2^128`; the deposit must reject
    /// before recording the transferred assets or minting truncated shares.
    #[test]
    fn prop_a031_deposit_payment_does_not_wrap_to_zero_shares(
        total_shares in Just(i128::MAX as u128),
        assets_in in Just(2u128),
    ) {
        let state = VaultState {
            total_assets: 0,
            total_shares,
            idle_assets: 0,
            ..VaultState::default()
        };
        let mut config = default_config();
        config.virtual_shares = 0;
        config.virtual_assets = 0;

        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::Deposit {
                owner: owner_addr(1),
                receiver: receiver_addr(1),
                assets_in,
                min_shares_out: 0,
                now_ns: TimestampNs(1),
            },
        );

        match result {
            Ok(result) => {
                let minted = result.effects.iter().find_map(|effect| match effect {
                    KernelEffect::MintShares { shares, .. } => Some(*shares),
                    _ => None,
                });
                prop_assert!(
                    minted.unwrap_or_default() > 0,
                    "nonzero asset payment minted zero shares after quotient truncation"
                );
            }
            Err(_) => {
                prop_assert!(true);
            }
        }
    }

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
            matches!(result, Err(TransitionError::WrongState)),
            "expected WrongState when starting allocation from non-idle"
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
            matches!(result, Err(TransitionError::WrongState)),
            "expected WrongState when starting withdrawal from non-idle"
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
            plan: vec![alloc_step(0, 100)],
        });
        let result = start_refresh(non_idle, plan, op_id);
        prop_assert!(
            matches!(result, Err(TransitionError::WrongState)),
            "expected WrongState when starting refresh from non-idle"
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
            request_id: op_id,
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
            request_id: op_id,
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
            request_id: request.request_id,
            index: 1,
            remaining: 0,
            collected: request.amount,
            receiver: request.receiver,
            owner: request.owner,
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
            request_id: op_id,
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
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), expected_assets);
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
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), expected);
        let settlement = EscrowSettlement::burn_all(entry.shares);
        prop_assert_eq!(settlement.to_burn, shares);
        prop_assert_eq!(settlement.refund, 0);
    }

    /// Property 63: settle_full_refund refunds all
    #[test]
    fn prop_settle_full_refund_all(
        shares in 0u128..=u64::MAX as u128,
        expected in 0u128..=u64::MAX as u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), expected);
        let settlement = EscrowSettlement::refund_all(entry.shares);
        prop_assert_eq!(settlement.to_burn, 0);
        prop_assert_eq!(settlement.refund, shares);
    }

    /// Property 64: apply_settlement validates bounds
    #[test]
    fn prop_apply_settlement_validates(
        shares in 1u128..=u64::MAX as u128 - 1,
        excess in 1u128..=1_000_000u128,
    ) {
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), 1000);
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
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), 1000);
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
                TimestampNs(i as u64),
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
        let entry = EscrowEntry::new(owner_addr(1), shares, TimestampNs(0), 1000);
        prop_assert_eq!(entry.is_empty(), shares == 0);
    }
}

// Deterministic Boundary / Edge Case Tests

use templar_vault_kernel::{
    convert_to_assets, preview_deposit_shares, preview_withdraw_assets, PayoutOutcome, VaultConfig,
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
    templar_vault_kernel::Address([99u8; 32])
}

proptest! {
    #[test]
    fn prop_asset_share_mutations_do_not_overflow_or_underflow(
        idle in any::<u128>(),
        total_shares in any::<u128>(),
        assets_in in any::<u128>(),
    ) {
        let mut state = default_state();
        state.total_assets = idle;
        state.idle_assets = idle;
        state.total_shares = total_shares;
        state.fee_anchor = FeeAccrualAnchor::new(idle, TimestampNs(0));
        let old_state = state.clone();
        let config = default_config();

        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::Deposit {
                owner: owner_addr(1),
                receiver: receiver_addr(1),
                assets_in,
                min_shares_out: 0,
                now_ns: TimestampNs(1),
            },
        );

        if let Ok(result) = result {
            prop_assert_eq!(
                result.state.total_assets.checked_sub(old_state.total_assets),
                Some(assets_in),
            );
            prop_assert_eq!(
                result.state.idle_assets.checked_sub(old_state.idle_assets),
                Some(assets_in),
            );
            prop_assert!(result.state.total_shares >= old_state.total_shares);
            prop_assert!(result.state.check_invariant());
        }
    }

    #[test]
    fn prop_atomic_withdraw_respects_idle_cap(
        idle in 1u128..=1_000_000_000_000u128,
        external in 0u128..=1_000_000_000_000u128,
        total_shares in 1u128..=1_000_000_000_000u128,
        assets_out in 1u128..=2_000_000_000_000u128,
    ) {
        let mut state = default_state();
        state.total_assets = idle + external;
        state.idle_assets = idle;
        state.external_assets = external;
        state.total_shares = total_shares;
        state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, TimestampNs(0));
        let old_state = state.clone();
        let config = default_config();
        let owner = owner_addr(1);
        let receiver = receiver_addr(1);

        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::AtomicWithdraw {
                owner,
                receiver,
                operator: owner,
                assets_out,
                max_shares_burned: u128::MAX,
                now_ns: TimestampNs(1),
            },
        );

        if assets_out > old_state.idle_assets {
            prop_assert!(result.is_err(), "atomic withdraw over idle cap succeeded");
            return Ok(());
        }

        if let Ok(result) = result {
            let burned_shares = result
                .effects
                .iter()
                .find_map(|effect| match effect {
                    KernelEffect::BurnShares { owner: effect_owner, shares }
                        if *effect_owner == owner => Some(*shares),
                    _ => None,
                })
                .expect("successful atomic withdraw must burn shares");
            let transferred_assets = result
                .effects
                .iter()
                .find_map(|effect| match effect {
                    KernelEffect::TransferAssets { to, amount } if *to == receiver => Some(*amount),
                    _ => None,
                })
                .expect("successful atomic withdraw must transfer assets");

            prop_assert_eq!(transferred_assets, assets_out);
            prop_assert!(burned_shares <= old_state.total_shares);
            prop_assert_eq!(result.state.idle_assets, old_state.idle_assets - assets_out);
            prop_assert_eq!(result.state.total_assets, old_state.total_assets - assets_out);
            prop_assert_eq!(
                result.state.total_shares,
                old_state.total_shares - burned_shares,
            );
            prop_assert!(result.state.check_invariant());
        }
    }

    #[test]
    fn prop_atomic_redeem_respects_idle_cap(
        idle in 1u128..=1_000_000_000_000u128,
        external in 0u128..=1_000_000_000_000u128,
        total_shares in 1u128..=1_000_000_000_000u128,
        shares in 1u128..=2_000_000_000_000u128,
    ) {
        let mut state = default_state();
        state.total_assets = idle + external;
        state.idle_assets = idle;
        state.external_assets = external;
        state.total_shares = total_shares;
        state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, TimestampNs(0));
        let old_state = state.clone();
        let config = default_config();
        let owner = owner_addr(1);
        let receiver = receiver_addr(1);
        let previewed_assets = preview_withdraw_assets(&old_state, &config, shares);

        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::AtomicRedeem {
                owner,
                receiver,
                operator: owner,
                shares,
                min_assets_out: 0,
                now_ns: TimestampNs(1),
            },
        );

        if previewed_assets > old_state.idle_assets {
            prop_assert!(result.is_err(), "atomic redeem over idle cap succeeded");
            return Ok(());
        }

        if let Ok(result) = result {
            let burned_shares = result
                .effects
                .iter()
                .find_map(|effect| match effect {
                    KernelEffect::BurnShares { owner: effect_owner, shares }
                        if *effect_owner == owner => Some(*shares),
                    _ => None,
                })
                .expect("successful atomic redeem must burn shares");
            let transferred_assets = result
                .effects
                .iter()
                .find_map(|effect| match effect {
                    KernelEffect::TransferAssets { to, amount } if *to == receiver => Some(*amount),
                    _ => None,
                })
                .expect("successful atomic redeem must transfer assets");

            prop_assert_eq!(burned_shares, shares);
            prop_assert_eq!(transferred_assets, previewed_assets);
            prop_assert!(burned_shares <= old_state.total_shares);
            prop_assert!(transferred_assets <= old_state.idle_assets);
            prop_assert_eq!(result.state.idle_assets, old_state.idle_assets - transferred_assets);
            prop_assert_eq!(result.state.total_assets, old_state.total_assets - transferred_assets);
            prop_assert_eq!(
                result.state.total_shares,
                old_state.total_shares - burned_shares,
            );
            prop_assert!(result.state.check_invariant());
        }
    }

    #[test]
    fn prop_request_withdraw_escrow_conservation(
        idle in 1u128..=1_000_000_000_000u128,
        external in 0u128..=1_000_000_000_000u128,
        total_shares in 1u128..=1_000_000_000_000u128,
        requested_shares in 1u128..=1_000_000_000_000u128,
    ) {
        let mut config = default_config();
        config.min_withdrawal_assets = 0;

        let mut state = default_state();
        state.total_assets = idle + external;
        state.idle_assets = idle;
        state.external_assets = external;
        state.total_shares = total_shares;
        state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, TimestampNs(0));
        let shares = requested_shares.min(total_shares);
        let expected_assets = convert_to_assets(&state, &config, shares);
        let old_state = state.clone();
        let owner = owner_addr(1);
        let receiver = receiver_addr(1);
        let vault = self_addr();

        let result = apply_action(
            state,
            &config,
            None,
            &vault,
            KernelAction::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out: 0,
                now_ns: TimestampNs(1),
            },
        )
        .expect("valid request withdraw should enqueue");

        prop_assert_eq!(result.state.total_assets, old_state.total_assets);
        prop_assert_eq!(result.state.idle_assets, old_state.idle_assets);
        prop_assert_eq!(result.state.external_assets, old_state.external_assets);
        prop_assert_eq!(result.state.total_shares, old_state.total_shares);
        prop_assert!(expected_assets <= old_state.total_assets);
        prop_assert!(result.state.check_invariant());

        let pending = result
            .state
            .withdraw_queue
            .pending_withdrawals()
            .values()
            .next()
            .expect("request should be queued");
        prop_assert_eq!(pending.escrow_shares, shares);
        prop_assert_eq!(pending.expected_assets, expected_assets);

        let transfer = result
            .effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::TransferShares { from, to, shares: effect_shares }
                    if *from == owner && *to == vault => Some(*effect_shares),
                _ => None,
            })
            .expect("request withdraw must escrow shares");
        prop_assert_eq!(transfer, shares);
    }
}

/// Boundary 1: Depositing zero assets returns ZeroAmount error.
#[test]
fn deposit_zero_assets_returns_zero_amount() {
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
            now_ns: TimestampNs(1),
        },
    );
    assert!(
        matches!(
            result,
            Err(templar_vault_kernel::error::KernelError::ZeroAmount)
        ),
        "Depositing 0 assets should return ZeroAmount, got: {result:?}",
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
            now_ns: TimestampNs(1),
        },
    );
    let result = result.expect("1 wei deposit should succeed");
    assert_eq!(result.state.total_assets, 1);
    assert_eq!(result.state.idle_assets, 1);
    assert!(
        result.state.total_shares > 0,
        "Should mint at least 1 share"
    );
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
    assert!(
        assets_back <= 1,
        "Round-trip should not inflate: got {assets_back}"
    );
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
            now_ns: TimestampNs(1),
        },
    );
    assert!(
        matches!(
            result,
            Err(templar_vault_kernel::error::KernelError::MinWithdrawal { .. })
        ),
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
            now_ns: TimestampNs(1),
        },
    );
    assert!(
        result.is_ok(),
        "Withdrawal at MIN should succeed, got: {result:?}"
    );
}

/// Boundary 8: Request withdraw with 0 shares returns ZeroAmount.
#[test]
fn withdraw_zero_shares_returns_zero_amount() {
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
            now_ns: TimestampNs(1),
        },
    );
    assert!(
        matches!(
            result,
            Err(templar_vault_kernel::error::KernelError::ZeroAmount)
        ),
        "Withdrawing 0 shares should return ZeroAmount, got: {result:?}",
    );
}

/// Boundary 9: Fee calculation with total_assets = 1.
#[test]
fn fee_shares_with_total_assets_one() {
    // With minimal total_assets, fee shares should be 0 or very small
    let fee_shares = compute_fee_shares(
        Number::from(1u128),                // current total_assets
        Number::from(0u128),                // last total_assets (0 → gain = 1)
        Wad::from(MAX_PERFORMANCE_FEE_WAD), // max performance fee
        Number::from(1u128),                // total_supply
    );
    // Fee shares should not exceed total supply
    assert!(
        fee_shares <= Number::from(1u128),
        "Fee shares {:?} should not exceed total supply of 1",
        fee_shares,
    );
}

/// Boundary 10: Queue at exactly MAX_QUEUE_LENGTH (alias of MAX_PENDING) rejects next enqueue.
#[test]
fn queue_at_max_rejects_enqueue() {
    assert!(
        can_enqueue(MAX_QUEUE_LENGTH - 1),
        "Should allow enqueue below max"
    );
    assert!(
        !can_enqueue(MAX_QUEUE_LENGTH),
        "Should reject enqueue at max"
    );
    assert!(
        !can_enqueue(MAX_QUEUE_LENGTH + 1),
        "Should reject enqueue above max"
    );
}

/// Boundary 11: is_valid_withdrawal_amount at boundary values.
#[test]
fn withdrawal_amount_boundary_values() {
    assert!(!is_valid_withdrawal_amount(0), "0 is not valid");
    assert!(
        !is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS - 1),
        "Below min is not valid"
    );
    assert!(
        is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS),
        "Exactly min is valid"
    );
    assert!(
        is_valid_withdrawal_amount(MIN_WITHDRAWAL_ASSETS + 1),
        "Above min is valid"
    );
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
        settlement.to_burn + settlement.refund,
        escrow,
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
            TimestampNs(i as u64),
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
        TimestampNs(max as u64),
        max,
    );
    assert!(result.is_err(), "Enqueue beyond capacity should fail",);
}

/// Boundary 15: Cooldown at exact boundary.
#[test]
fn cooldown_exact_boundary() {
    let cooldown_ns = 1_000_000u64;
    let requested_at = 100u64;

    // Just before cooldown
    assert!(
        !is_past_cooldown(
            TimestampNs(requested_at),
            TimestampNs(requested_at + cooldown_ns - 1),
            cooldown_ns,
        ),
        "Should NOT be past cooldown 1ns before",
    );
    // Exactly at cooldown
    assert!(
        is_past_cooldown(
            TimestampNs(requested_at),
            TimestampNs(requested_at + cooldown_ns),
            cooldown_ns,
        ),
        "Should be past cooldown at exact boundary",
    );
    // Just after cooldown
    assert!(
        is_past_cooldown(
            TimestampNs(requested_at),
            TimestampNs(requested_at + cooldown_ns + 1),
            cooldown_ns,
        ),
        "Should be past cooldown 1ns after",
    );
}

/// Boundary 16: Zero cooldown means immediately ready when now >= requested_at.
#[test]
fn zero_cooldown_passes_when_now_gte_requested() {
    assert!(
        is_past_cooldown(TimestampNs(0), TimestampNs(0), 0),
        "Zero cooldown, same time → past"
    );
    assert!(
        is_past_cooldown(TimestampNs(100), TimestampNs(100), 0),
        "Zero cooldown, same time → past"
    );
    assert!(
        is_past_cooldown(TimestampNs(100), TimestampNs(101), 0),
        "Zero cooldown, later now → past"
    );
    assert!(
        !is_past_cooldown(TimestampNs(100), TimestampNs(99), 0),
        "Zero cooldown, earlier now → not past (request not yet made)"
    );
}

// Overflow / Saturation Tests

/// Overflow 1: Deposit near u128::MAX should reject instead of saturating.
#[test]
fn deposit_near_max_rejected() {
    let mut state = default_state();
    state.total_assets = u128::MAX - 10;
    state.idle_assets = u128::MAX - 10;
    state.total_shares = u128::MAX / 2;
    let config = default_config();

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: receiver_addr(1),
            assets_in: 100, // Would overflow total_assets
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    );
    assert!(matches!(
        result,
        Err(templar_vault_kernel::error::KernelError::InvalidState(
            templar_vault_kernel::error::InvalidStateCode::DepositOverflowTotalAssets
        ))
    ));
}

/// Overflow 2: Fee calculation with extreme values doesn't panic.
#[test]
fn fee_shares_extreme_values_no_panic() {
    // Large total_assets with significant gain
    let fee_shares = compute_fee_shares(
        Number::from(u64::MAX as u128),     // current_total_assets
        Number::from(u64::MAX as u128 / 2), // last_total_assets (50% gain)
        Wad::from(MAX_PERFORMANCE_FEE_WAD), // max fee
        Number::from(u64::MAX as u128),     // total_supply
    );
    // Should not panic; just verify it returns some value
    let _ = fee_shares;
}

/// Overflow 3: Fee calculation at u128 boundary.
#[test]
fn fee_shares_u128_max_no_panic() {
    let fee_shares = compute_fee_shares(
        Number::from(u128::MAX / 2), // current
        Number::from(u128::MAX / 4), // last (significant gain)
        Wad::from(MAX_PERFORMANCE_FEE_WAD),
        Number::from(u128::MAX / 2), // total_supply
    );
    let _ = fee_shares;
}

/// Overflow 4: SyncExternalAssets at u128::MAX saturates total_assets.
#[test]
fn sync_external_near_max_saturates() {
    let mut state = default_state();
    state.idle_assets = u128::MAX / 2;
    state.total_assets = u128::MAX / 2;
    state.total_shares = 1_000_000;

    // Start an allocation to get into a state where sync is allowed
    let alloc_result = start_allocation(
        state.op_state.clone(),
        vec![alloc_step(0, 1000)],
        state.next_op_id,
    )
    .expect("allocation should start");
    state.op_state = alloc_result.new_state;

    let config = default_config();
    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::SyncExternalAssets {
            new_external_assets: u128::MAX, // Would overflow with idle_assets
            op_id: 0,
            now_ns: TimestampNs(1),
        },
    );
    // Should fail: idle + MAX would overflow u128
    assert!(result.is_err(), "SyncExternalAssets should reject overflow",);
}

/// Overflow 5: Preview deposit with u128::MAX assets doesn't panic.
#[test]
fn preview_deposit_max_assets_no_panic() {
    let mut state = default_state();
    state.total_assets = 1_000_000;
    state.total_shares = 1_000_000;
    let config = default_config();

    // Should not panic even with extreme input
    let shares = preview_deposit_shares(&state, &config, u128::MAX);
    let _ = shares;
}

/// Overflow 6: Preview withdraw with u128::MAX shares doesn't panic.
#[test]
fn preview_withdraw_max_shares_no_panic() {
    let mut state = default_state();
    state.total_assets = 1_000_000;
    state.total_shares = 1_000_000;
    let config = default_config();

    let assets = preview_withdraw_assets(&state, &config, u128::MAX);
    let _ = assets;
}

/// Overflow 7: compute_settlement with u128::MAX values preserves conservation.
#[test]
fn settlement_u128_max_conservation() {
    let escrow = u128::MAX;
    let expected = u128::MAX;
    let actual = u128::MAX;

    let settlement = compute_settlement(escrow, expected, actual);
    assert_eq!(
        settlement.to_burn.saturating_add(settlement.refund),
        escrow,
        "Conservation must hold even at u128::MAX",
    );
}

/// Overflow 8: compute_settlement with extreme disparity.
#[test]
fn settlement_extreme_disparity() {
    // Tiny actual relative to huge expected
    let escrow = u128::MAX;
    let expected = u128::MAX;
    let actual = 1u128;

    let settlement = compute_settlement(escrow, expected, actual);
    assert!(
        settlement.to_burn >= 1,
        "Should burn at least 1 with 1 wei actual"
    );
    assert_eq!(
        settlement.to_burn + settlement.refund,
        escrow,
        "Conservation: burn + refund = escrow",
    );
}

/// Overflow 9: mul_div_floor with large values doesn't panic.
#[test]
fn mul_div_floor_large_values_no_panic() {
    let result = mul_div_floor(
        Number::from(u128::MAX),
        Number::from(u128::MAX),
        Number::from(u128::MAX),
    );
    // MAX * MAX / MAX = MAX (approximately)
    assert!(u128::from(result) > 0, "Should produce non-zero result");
}

/// Overflow 10: Cooldown with u64::MAX timestamp doesn't panic.
/// saturating_add clamps overflow to MAX, so requested_at=MAX + cooldown=1 → MAX.
#[test]
fn cooldown_u64_max_no_panic() {
    // requested_at=MAX, cooldown=1 → saturates to MAX; now=MAX >= MAX → true
    assert!(
        is_past_cooldown(TimestampNs(u64::MAX), TimestampNs(u64::MAX), 1),
        "Saturating add clamps to MAX, so passes"
    );
    assert!(
        is_past_cooldown(TimestampNs(u64::MAX), TimestampNs(u64::MAX), 0),
        "Zero cooldown at MAX should pass"
    );
    assert!(
        is_past_cooldown(TimestampNs(0), TimestampNs(u64::MAX), u64::MAX),
        "Should be past when now=MAX, cooldown=MAX"
    );
    // now=0 is before requested_at=MAX, so not past cooldown
    assert!(
        !is_past_cooldown(TimestampNs(u64::MAX), TimestampNs(0), 1),
        "now=0 before requested_at=MAX"
    );
}

/// AddressBook: insert and resolve round-trips correctly.
#[test]
fn address_book_insert_resolve() {
    use templar_vault_kernel::AddressBook;
    let mut book = AddressBook::<&str>::new();
    let addr_a: [u8; 32] = [1u8; 32];
    let addr_b: [u8; 32] = [2u8; 32];

    book.insert(Address(addr_a), "alice");
    book.insert(Address(addr_b), "bob");

    assert_eq!(book.resolve(&Address(addr_a)), Some(&"alice"));
    assert_eq!(book.resolve(&Address(addr_b)), Some(&"bob"));
    assert_eq!(book.len(), 2);
}

/// AddressBook: inserting same key overwrites (no silent collision).
#[test]
fn address_book_overwrite_same_key() {
    use templar_vault_kernel::AddressBook;
    let mut book = AddressBook::<&str>::new();
    let addr: [u8; 32] = [1u8; 32];

    book.insert(Address(addr), "alice");
    book.insert(Address(addr), "bob");

    assert_eq!(
        book.resolve(&Address(addr)),
        Some(&"bob"),
        "Last insert wins"
    );
    assert_eq!(book.len(), 1, "No duplicate entries");
}

/// AddressBook: distinct 32-byte addresses never shadow each other.
#[test]
fn address_book_distinct_addresses_no_collision() {
    use templar_vault_kernel::AddressBook;
    let mut book = AddressBook::<u32>::new();

    for i in 0u8..=255 {
        let mut addr = [0u8; 32];
        addr[0] = i;
        book.insert(Address(addr), i as u32);
    }

    assert_eq!(book.len(), 256, "256 distinct single-byte-varied addresses");

    for i in 0u8..=255 {
        let mut addr = [0u8; 32];
        addr[0] = i;
        assert_eq!(book.resolve(&Address(addr)), Some(&(i as u32)));
    }
}

/// AddressBook: resolving nonexistent address returns None.
#[test]
fn address_book_missing_returns_none() {
    use templar_vault_kernel::AddressBook;
    let book = AddressBook::<&str>::new();
    assert_eq!(book.resolve(&Address([42u8; 32])), None);
    assert!(book.is_empty());
}

/// Performance fee when profit is less than fee denominator floors to zero shares.
#[test]
fn fee_zero_when_profit_below_fee_threshold() {
    // Tiny profit (1 wei), 50% fee → fee_assets = floor(1 * 0.5) = 0 → 0 shares
    let fee_shares = compute_fee_shares(
        Number::from(1_000_001u128),        // cur_total_assets
        Number::from(1_000_000u128),        // last_total_assets → profit = 1
        Wad::from(MAX_PERFORMANCE_FEE_WAD), // 50%
        Number::from(1_000_000u128),        // total_supply
    );
    // With profit=1, fee_assets = floor(1 * 0.5) = 0, so fee_shares = 0
    assert_eq!(
        u128::from(fee_shares),
        0,
        "Sub-threshold profit should yield zero fee shares"
    );
}

/// No profit (cur <= last) → zero fee shares regardless of fee rate.
#[test]
fn fee_zero_when_no_profit() {
    // cur == last → profit = 0
    let fee_shares = compute_fee_shares(
        Number::from(1_000_000u128),
        Number::from(1_000_000u128),
        Wad::from(MAX_PERFORMANCE_FEE_WAD),
        Number::from(1_000_000u128),
    );
    assert_eq!(u128::from(fee_shares), 0, "No profit → no fee shares");

    // cur < last → profit = 0 (saturating_sub)
    let fee_shares_loss = compute_fee_shares(
        Number::from(500_000u128),
        Number::from(1_000_000u128),
        Wad::from(MAX_PERFORMANCE_FEE_WAD),
        Number::from(1_000_000u128),
    );
    assert_eq!(u128::from(fee_shares_loss), 0, "Loss → no fee shares");
}

/// Combined fee_assets equaling cur_total_assets → zero shares (denom = 0 case).
#[test]
fn fee_zero_when_fee_consumes_all_assets() {
    // If fee_assets == cur_total_assets, compute_fee_shares_from_assets returns 0
    let fee_shares = compute_fee_shares_from_assets(
        Number::from(1_000u128), // fee_assets = all of total
        Number::from(1_000u128), // cur_total_assets
        Number::from(1_000u128), // total_supply
    );
    assert_eq!(
        u128::from(fee_shares),
        0,
        "Fee consuming all assets must produce zero shares"
    );
}

/// Fee_assets exceeding cur_total_assets → zero shares.
#[test]
fn fee_zero_when_fee_exceeds_total_assets() {
    let fee_shares = compute_fee_shares_from_assets(
        Number::from(2_000u128), // fee_assets > total
        Number::from(1_000u128), // cur_total_assets
        Number::from(1_000u128), // total_supply
    );
    assert_eq!(
        u128::from(fee_shares),
        0,
        "Fee exceeding total assets must produce zero shares"
    );
}

/// MAX_PERFORMANCE_FEE_WAD (50%) extracts correct proportion.
#[test]
fn fee_at_max_performance_rate() {
    let total = 2_000_000u128;
    let profit = 1_000_000u128;
    let supply = 1_000_000u128;
    let fee_shares = compute_fee_shares(
        Number::from(total),
        Number::from(total - profit),
        Wad::from(MAX_PERFORMANCE_FEE_WAD), // 50%
        Number::from(supply),
    );
    // fee_assets = floor(profit * 0.5) = 500_000
    // denom = total - fee_assets = 1_500_000
    // fee_shares = floor(500_000 * 1_000_000 / 1_500_000) = 333_333
    let expected = 500_000u128 * supply / (total - 500_000);
    assert_eq!(
        u128::from(fee_shares),
        expected,
        "50% fee on 1M profit with 2M total should mint {expected} shares"
    );
}

/// 100% fee rate (Wad::one()) → fee_assets = profit → denom = total - profit.
/// If profit == total, fee_assets == total → 0 shares.
#[test]
fn fee_at_100_percent_rate() {
    // 100% fee, profit == total → fee_assets == total → zero shares
    let fee_shares_all = compute_fee_shares(
        Number::from(1_000_000u128),
        Number::from(0u128),
        Wad::one(), // 100%
        Number::from(1_000_000u128),
    );
    assert_eq!(
        u128::from(fee_shares_all),
        0,
        "100% fee on profit==total should yield 0 (denom becomes 0)"
    );

    // 100% fee, profit < total → fee_assets = profit, denom = total - profit > 0
    let fee_shares_partial = compute_fee_shares(
        Number::from(2_000_000u128),
        Number::from(1_000_000u128),
        Wad::one(), // 100%
        Number::from(1_000_000u128),
    );
    // fee_assets = 1_000_000, denom = 1_000_000
    // fee_shares = floor(1_000_000 * 1_000_000 / 1_000_000) = 1_000_000
    assert_eq!(
        u128::from(fee_shares_partial),
        1_000_000,
        "100% fee on partial profit should mint shares equal to supply ratio"
    );
}

/// Fee with zero total supply → always zero shares.
#[test]
fn fee_zero_on_zero_supply() {
    let fee_shares = compute_fee_shares(
        Number::from(2_000_000u128),
        Number::from(1_000_000u128),
        Wad::from(MAX_PERFORMANCE_FEE_WAD),
        Number::from(0u128), // no supply
    );
    assert_eq!(u128::from(fee_shares), 0, "Zero supply → zero fee shares");
}

/// Fee anchor timestamp wraparound: RefreshFees rejects backwards time.
#[test]
fn fee_refresh_rejects_backwards_timestamp() {
    let config = default_config();
    let mut state = default_state();
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(10_000));

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(5_000),
        },
    );
    assert!(result.is_err(), "Backwards timestamp must be rejected");
}

/// Fee anchor updates correctly on RefreshFees.
#[test]
fn fee_refresh_updates_anchor() {
    let config = default_config();
    let mut state = default_state();
    state.total_assets = 5_000;
    state.fee_anchor = FeeAccrualAnchor::new(1_000, TimestampNs(100));

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(200),
        },
    )
    .expect("Forward timestamp should succeed");

    assert_eq!(result.state.fee_anchor.total_assets, 5_000);
    assert_eq!(result.state.fee_anchor.timestamp_ns, TimestampNs(200));
}

/// Fee anchor at timestamp 0 → RefreshFees at 0 is rejected (must advance).
#[test]
fn fee_refresh_at_zero_timestamp() {
    let config = default_config();
    let state = default_state(); // fee_anchor at (0, 0)

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RefreshFees {
            now_ns: TimestampNs(0),
        },
    );
    assert!(
        result.is_err(),
        "RefreshFees at timestamp 0 should reject non-advancing time"
    );
}

/// MAX_MANAGEMENT_FEE_WAD constant is 5% (sanity check).
#[test]
fn management_fee_cap_constant() {
    assert_eq!(
        MAX_MANAGEMENT_FEE_WAD,
        Wad::SCALE / 100 * 5,
        "MAX_MANAGEMENT_FEE_WAD should be 5%"
    );
}

/// MAX_PERFORMANCE_FEE_WAD constant is 50% (sanity check).
#[test]
fn performance_fee_cap_constant() {
    assert_eq!(
        MAX_PERFORMANCE_FEE_WAD,
        Wad::SCALE / 100 * 50,
        "MAX_PERFORMANCE_FEE_WAD should be 50%"
    );
}

/// Build a queue with `n` pending withdrawals, each of `assets` expected.
fn build_large_queue(n: u32, assets_per: u128) -> WithdrawQueue {
    let mut queue = WithdrawQueue::new();
    for i in 0..n {
        let mut owner = [0u8; 32];
        owner[..4].copy_from_slice(&i.to_le_bytes());
        queue
            .enqueue(
                Address(owner),
                Address(owner),
                assets_per,            // escrow_shares
                assets_per,            // expected_assets
                TimestampNs(i as u64), // requested_at_ns
                MAX_PENDING as u32,
            )
            .unwrap_or_else(|e| panic!("enqueue {i} failed: {e:?}"));
    }
    queue
}

/// Queue at MAX_PENDING capacity: enqueue fills, then rejects.
#[test]
fn queue_fills_to_max_pending_then_rejects() {
    let queue = build_large_queue(MAX_PENDING as u32, 1_000);
    assert_eq!(
        queue.pending_withdrawals().len(),
        MAX_PENDING,
        "Queue should hold exactly MAX_PENDING items"
    );
    // Next enqueue should fail
    let mut queue = queue;
    let result = queue.enqueue(
        Address([255u8; 32]),
        Address([255u8; 32]),
        1_000,
        1_000,
        TimestampNs(9999),
        MAX_PENDING as u32,
    );
    assert!(
        result.is_err(),
        "Should reject enqueue at MAX_PENDING capacity"
    );
}

/// count_satisfiable at MAX_PENDING depth: all satisfiable when enough assets.
#[test]
fn count_satisfiable_at_max_pending() {
    let n = MAX_PENDING as u32;
    let assets_per = 1_000u128;
    let queue = build_large_queue(n, assets_per);
    let items: Vec<_> = queue.pending_withdrawals().values().collect();

    // Enough assets to satisfy all
    let available = n as u128 * assets_per;
    let (count, total) = count_satisfiable(items.iter().copied(), available);
    assert_eq!(count, n, "All {n} items should be satisfiable");
    assert_eq!(total, available, "Total should equal all items");
}

/// count_satisfiable at MAX_PENDING depth: partial satisfaction.
#[test]
fn count_satisfiable_partial_at_max_pending() {
    let n = MAX_PENDING as u32;
    let assets_per = 1_000u128;
    let queue = build_large_queue(n, assets_per);
    let items: Vec<_> = queue.pending_withdrawals().values().collect();

    // Only enough for half
    let half = n / 2;
    let available = half as u128 * assets_per;
    let (count, total) = count_satisfiable(items.iter().copied(), available);
    assert_eq!(
        count, half,
        "Half items should be satisfiable with half assets"
    );
    assert_eq!(total, available);
}

/// compute_queue_status at MAX_PENDING depth: correct totals.
#[test]
fn queue_status_at_max_pending() {
    let n = MAX_PENDING as u32;
    let assets_per = 1_000u128;
    let queue = build_large_queue(n, assets_per);
    let items: Vec<_> = queue.pending_withdrawals().values().collect();

    let status = compute_queue_status(items.iter().copied());
    assert_eq!(status.length, n, "Length should be MAX_PENDING");
    assert_eq!(
        status.total_expected_assets,
        n as u128 * assets_per,
        "Total expected assets"
    );
    assert_eq!(
        status.total_escrow_shares,
        n as u128 * assets_per,
        "Total escrow shares"
    );
}

/// find_request_status at MAX_PENDING depth: find last item (worst case O(n)).
#[test]
fn find_request_status_worst_case_at_max_pending() {
    use templar_vault_kernel::state::queue::find_request_status;
    let n = MAX_PENDING as u32;
    let assets_per = 1_000u128;
    let queue = build_large_queue(n, assets_per);
    let items: Vec<_> = queue.pending_withdrawals().values().collect();

    // Find the last owner (worst-case linear scan)
    let mut last_owner = [0u8; 32];
    last_owner[..4].copy_from_slice(&(n - 1).to_le_bytes());

    let status = find_request_status(items.iter().copied(), &Address(last_owner));
    assert!(status.is_some(), "Last owner should be found");
    let status = status.unwrap();
    assert_eq!(status.index, n - 1, "Should be at the last position");
    assert_eq!(
        status.depth_assets,
        (n as u128 - 1) * assets_per,
        "Depth should be sum of all preceding items"
    );
}

/// find_request_status at MAX_PENDING depth: owner not found (full scan).
#[test]
fn find_request_status_miss_at_max_pending() {
    use templar_vault_kernel::state::queue::find_request_status;
    let n = MAX_PENDING as u32;
    let queue = build_large_queue(n, 1_000);
    let items: Vec<_> = queue.pending_withdrawals().values().collect();

    // Owner that doesn't exist
    let missing_owner = [255u8; 32];
    let status = find_request_status(items.iter().copied(), &Address(missing_owner));
    assert!(status.is_none(), "Non-existent owner should return None");
}

/// Queue enqueue/dequeue cycle at high volume: enqueue MAX_PENDING, dequeue half, re-enqueue.
#[test]
fn queue_churn_at_high_depth() {
    let n = MAX_PENDING as u32;
    let assets_per = 500u128;
    let mut queue = build_large_queue(n, assets_per);
    assert_eq!(queue.pending_withdrawals().len(), n as usize);

    // Dequeue half from the front
    let half = n / 2;
    for _ in 0..half {
        let _ = queue.dequeue();
    }
    assert_eq!(queue.pending_withdrawals().len(), (n - half) as usize);

    // Re-enqueue to fill back up
    for i in 0..half {
        let mut owner = [128u8; 32];
        owner[..4].copy_from_slice(&i.to_le_bytes());
        queue
            .enqueue(
                Address(owner),
                Address(owner),
                assets_per,
                assets_per,
                TimestampNs(10_000 + i as u64),
                n,
            )
            .unwrap_or_else(|e| panic!("re-enqueue {i} failed: {e:?}"));
    }
    assert_eq!(
        queue.pending_withdrawals().len(),
        n as usize,
        "Should be full again"
    );

    // Verify queue status is correct after churn
    let items: Vec<_> = queue.pending_withdrawals().values().collect();
    let status = compute_queue_status(items.iter().copied());
    assert_eq!(status.length, n);
    assert_eq!(status.total_expected_assets, n as u128 * assets_per);
}

use primitive_types::U256;

/// mul_div_floor with U256::MAX inputs: MAX * MAX / MAX = MAX.
#[test]
fn mul_div_floor_u256_max_all() {
    let max_n = Number(U256::MAX);
    let result = Number::mul_div_floor(max_n, max_n, max_n);
    assert_eq!(result.0, U256::MAX, "MAX * MAX / MAX should be MAX");
}

/// mul_div_floor: MAX * 1 / 1 = MAX.
#[test]
fn mul_div_floor_u256_max_times_one() {
    let max_n = Number(U256::MAX);
    let result = Number::mul_div_floor(max_n, Number::one(), Number::one());
    assert_eq!(result.0, U256::MAX, "MAX * 1 / 1 should be MAX");
}

/// mul_div_floor: 1 * 1 / MAX = 0 (floor).
#[test]
fn mul_div_floor_one_over_max() {
    let result = Number::mul_div_floor(Number::one(), Number::one(), Number(U256::MAX));
    assert!(result.is_zero(), "1 * 1 / MAX should floor to 0");
}

/// mul_div_ceil: 1 * 1 / MAX = 1 (ceil).
#[test]
fn mul_div_ceil_one_over_max() {
    let result = Number::mul_div_ceil(Number::one(), Number::one(), Number(U256::MAX));
    assert_eq!(result.0, U256::one(), "ceil(1 * 1 / MAX) should be 1");
}

/// mul_div_floor: MAX * MAX / 1 uses denom==1 fast path (saturating_mul).
#[test]
fn mul_div_floor_max_squared_div_one() {
    let max_n = Number(U256::MAX);
    let result = Number::mul_div_floor(max_n, max_n, Number::one());
    // Fast path: denom==1 → x.0.saturating_mul(y.0) = U256::MAX
    assert_eq!(
        result.0,
        U256::MAX,
        "MAX * MAX / 1 saturates to MAX via fast path"
    );
}

/// mul_div with zero operands: all combinations of zero produce zero.
#[test]
fn mul_div_zero_combinations() {
    let z = Number::zero();
    let one = Number::one();
    let max_n = Number(U256::MAX);

    // Zero x
    assert!(Number::mul_div_floor(z, max_n, one).is_zero());
    assert!(Number::mul_div_ceil(z, max_n, one).is_zero());
    // Zero y
    assert!(Number::mul_div_floor(max_n, z, one).is_zero());
    assert!(Number::mul_div_ceil(max_n, z, one).is_zero());
    // Zero denom (returns 0 by convention, not panic)
    assert!(Number::mul_div_floor(max_n, max_n, z).is_zero());
    assert!(Number::mul_div_ceil(max_n, max_n, z).is_zero());
    // All zero
    assert!(Number::mul_div_floor(z, z, z).is_zero());
    assert!(Number::mul_div_ceil(z, z, z).is_zero());
}

/// saturating_add: U256::MAX + U256::MAX saturates to MAX.
#[test]
fn saturating_add_u256_max() {
    let max_n = Number(U256::MAX);
    let result = max_n.saturating_add(max_n);
    assert_eq!(result.0, U256::MAX, "MAX + MAX should saturate to MAX");
}

/// saturating_sub: 0 - MAX saturates to 0.
#[test]
fn saturating_sub_zero_minus_max() {
    let result = Number::zero().saturating_sub(Number(U256::MAX));
    assert!(result.is_zero(), "0 - MAX should saturate to 0");
}

/// as_u128_saturating for values in the U256 range above u128::MAX.
#[test]
fn as_u128_saturating_boundary() {
    // Exactly u128::MAX should return u128::MAX
    let at_max = Number::from(u128::MAX);
    assert_eq!(at_max.as_u128_saturating(), u128::MAX);

    // One above u128::MAX should saturate
    let above = Number(U256::from(u128::MAX) + U256::one());
    assert_eq!(above.as_u128_saturating(), u128::MAX);

    // U256::MAX should saturate
    let way_above = Number(U256::MAX);
    assert_eq!(way_above.as_u128_saturating(), u128::MAX);
}

/// Wad::apply_floored with pathological inputs doesn't panic.
#[test]
fn wad_apply_floored_u128_max() {
    let max_amount = Number::from(u128::MAX);
    // 100% fee on MAX amount
    let result = Wad::one().apply_floored(max_amount);
    assert_eq!(result, max_amount, "100% of MAX should be MAX");

    // 50% fee on MAX
    let half_wad = Wad::from(Wad::SCALE / 2);
    let half_result = half_wad.apply_floored(max_amount);
    let expected: u128 = u128::MAX / 2;
    assert!(
        u128::from(half_result) >= expected - 1 && u128::from(half_result) <= expected,
        "50% of MAX should be approximately MAX/2, got {:?}",
        half_result
    );
}

/// Wad::apply_floored with Wad > 1.0 (super-WAD) produces result > input.
#[test]
fn wad_apply_floored_super_wad() {
    let double_wad = Wad::from(Wad::SCALE * 2); // 200%
    let amount = Number::from(1_000_000u128);
    let result = double_wad.apply_floored(amount);
    assert_eq!(
        u128::from(result),
        2_000_000,
        "200% WAD should double the amount"
    );
}

/// compute_fee_shares with every argument at u128::MAX: no panic.
#[test]
fn compute_fee_shares_all_max() {
    let max = Number::from(u128::MAX);
    // cur = MAX, last = 0 → profit = MAX
    // fee = 100% → fee_assets = MAX
    // fee_assets >= cur → returns 0
    let result = compute_fee_shares(max, Number::zero(), Wad::one(), max);
    assert_eq!(
        u128::from(result),
        0,
        "100% fee on profit==total should be 0"
    );
}

/// compute_fee_shares_from_assets with fee_assets = 1, total = u128::MAX.
#[test]
fn compute_fee_shares_from_assets_minimal_fee() {
    let total = Number::from(u128::MAX);
    let supply = Number::from(u128::MAX);
    let fee_assets = Number::from(1u128);
    // denom = MAX - 1, fee_shares = floor(1 * MAX / (MAX-1)) = 1
    let result = compute_fee_shares_from_assets(fee_assets, total, supply);
    assert_eq!(
        u128::from(result),
        1,
        "Minimal fee on max total should mint 1 share"
    );
}

/// Wad division: Wad::one() / 3 rounds down.
#[test]
fn wad_division_rounds_down() {
    let third = Wad::one() / 3;
    let expected = Wad::SCALE / 3;
    assert_eq!(u128::from(third), expected);
    // Verify it rounds down: expected * 3 < SCALE
    assert!(expected * 3 < Wad::SCALE, "Division should round down");
}

/// mul_div_floor fast path: x == denom returns y exactly.
#[test]
fn mul_div_floor_cancellation_paths() {
    let x = Number::from(12345u128);
    let y = Number::from(67890u128);
    // x * y / x = y
    assert_eq!(Number::mul_div_floor(x, y, x), y);
    // y * x / x = y
    assert_eq!(Number::mul_div_floor(y, x, x), y);
    // x * x / x = x
    assert_eq!(Number::mul_div_floor(x, x, x), x);
}

/// Allocation step failure at step 2 of 5: returns to Idle with correct total_allocated.
#[test]
fn allocation_step_failure_mid_plan() {
    let plan = vec![
        alloc_step(0, 100),
        alloc_step(1, 200),
        alloc_step(2, 300),
        alloc_step(3, 400),
        alloc_step(4, 500),
    ];
    let op_id = 1;
    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

    // Step 0 succeeds with 100
    let result = allocation_step_callback(result.new_state, true, 100, op_id).unwrap();
    assert!(matches!(result.new_state, OpState::Allocating(ref s) if s.index == 1));

    // Step 1 succeeds with 200
    let result = allocation_step_callback(result.new_state, true, 200, op_id).unwrap();
    assert!(matches!(result.new_state, OpState::Allocating(ref s) if s.index == 2));

    // Step 2 FAILS
    let result = allocation_step_callback(result.new_state, false, 0, op_id).unwrap();
    assert!(
        matches!(result.new_state, OpState::Idle),
        "Should return to Idle on failure"
    );

    // Verify the failure event contains correct total_allocated
    let event = &result.effects[0];
    match event {
        KernelEffect::EmitEvent {
            event:
                KernelEvent::AllocationStepFailed {
                    op_id: eid,
                    index,
                    remaining,
                    total_allocated,
                },
        } => {
            assert_eq!(*eid, op_id);
            assert_eq!(*index, 2, "Failed at step 2");
            assert_eq!(*remaining, 1200, "remaining = 1500 - 100 - 200 = 1200");
            assert_eq!(*total_allocated, 300, "allocated = 100 + 200 = 300");
        }
        _ => panic!("Expected AllocationStepFailed event"),
    }
}

/// Allocation step with amount = 0 on success is rejected.
#[test]
fn allocation_step_zero_amount_rejected() {
    let plan = vec![alloc_step(0, 100), alloc_step(1, 200)];
    let op_id = 1;
    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

    let err = allocation_step_callback(result.new_state, true, 0, op_id);
    assert!(
        matches!(err, Err(TransitionError::ZeroAllocationAmount)),
        "Zero allocation amount on success should be rejected, got: {err:?}"
    );
}

/// Allocation step with amount exceeding remaining is rejected.
#[test]
fn allocation_step_overflow_rejected() {
    let plan = vec![alloc_step(0, 100)];
    let op_id = 1;
    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

    let err = allocation_step_callback(result.new_state, true, 101, op_id);
    assert!(
        matches!(err, Err(TransitionError::AllocationOverflow { .. })),
        "Overflow amount should be rejected, got: {err:?}"
    );
}

/// AbortAllocating: returns to Idle and adds `restore_idle` back to idle_assets.
///
/// By design the kernel does NOT decrement `idle_assets` on `BeginAllocating`.
/// Idle-asset accounting is the executor's responsibility — each chain executor
/// (Soroban, NEAR) decrements `idle_assets` *before* calling into the kernel,
/// so the kernel never sees the pre-decrement value.
///
/// When testing the kernel in isolation (no executor wrapper), `idle_assets`
/// stays unchanged through `BeginAllocating` and then `AbortAllocating` adds
/// `restore_idle` on top, producing a value larger than the original. This is
/// expected kernel-only behavior, not a bug.
#[test]
fn abort_allocating_restores_state() {
    let config = default_config();
    let mut state = default_state();
    state.idle_assets = 1500;
    state.total_assets = 1500;

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: vec![alloc_step(0, 500), alloc_step(1, 500), alloc_step(2, 500)],
            now_ns: TimestampNs(0),
        },
    )
    .unwrap();
    let op_id = match &result.state.op_state {
        OpState::Allocating(s) => s.op_id,
        _ => panic!("Should be Allocating"),
    };
    // Kernel decrements idle_assets by allocation total (1500).
    assert_eq!(
        result.state.idle_assets, 0,
        "idle_assets decremented by allocation total"
    );
    assert_eq!(
        result.state.total_assets, 0,
        "total_assets recomputed after decrement"
    );
    let state_after_begin = result.state;

    let result = apply_action(
        state_after_begin,
        &config,
        None,
        &self_addr(),
        KernelAction::AbortAllocating { op_id },
    )
    .unwrap();

    assert!(matches!(result.state.op_state, OpState::Idle));
    // AbortAllocating restores the decremented amount, bringing us back to 1500.
    assert_eq!(
        result.state.idle_assets, 1500,
        "idle_assets restored after abort"
    );
}

/// Start allocation with empty plan is rejected.
#[test]
fn allocation_empty_plan_rejected() {
    let err = start_allocation(OpState::Idle, vec![], 1);
    assert!(
        matches!(err, Err(TransitionError::EmptyAllocationPlan)),
        "Empty plan should be rejected, got: {err:?}"
    );
}

/// Start allocation when not Idle is rejected.
#[test]
fn allocation_from_non_idle_rejected() {
    let alloc_state = OpState::Allocating(AllocatingState {
        op_id: 1,
        index: 0,
        remaining: 100,
        plan: vec![alloc_step(0, 100)],
    });
    let err = start_allocation(alloc_state, vec![alloc_step(0, 100)], 2);
    assert!(
        matches!(err, Err(TransitionError::WrongState)),
        "Allocation from non-Idle should be rejected, got: {err:?}"
    );
}

/// Allocation step failure at first step (step 0): total_allocated = 0.
#[test]
fn allocation_failure_at_first_step() {
    let plan = vec![alloc_step(0, 1000), alloc_step(1, 2000)];
    let op_id = 1;
    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

    // Step 0 fails immediately
    let result = allocation_step_callback(result.new_state, false, 0, op_id).unwrap();
    assert!(matches!(result.new_state, OpState::Idle));

    match &result.effects[0] {
        KernelEffect::EmitEvent {
            event:
                KernelEvent::AllocationStepFailed {
                    total_allocated,
                    remaining,
                    ..
                },
        } => {
            assert_eq!(*total_allocated, 0, "No steps completed → 0 allocated");
            assert_eq!(*remaining, 3000, "Full plan amount still remaining");
        }
        _ => panic!("Expected AllocationStepFailed event"),
    }
}

/// Allocation step with wrong op_id is rejected.
#[test]
fn allocation_step_wrong_op_id_rejected() {
    let plan = vec![alloc_step(0, 100)];
    let result = start_allocation(OpState::Idle, plan, 1).unwrap();
    let err = allocation_step_callback(result.new_state, true, 100, 999);
    assert!(
        matches!(err, Err(TransitionError::OpIdMismatch { .. })),
        "Wrong op_id should be rejected, got: {err:?}"
    );
}

/// Full allocation completes all steps and transitions to Idle.
#[test]
fn allocation_full_completion() {
    let plan = vec![alloc_step(0, 100), alloc_step(1, 200), alloc_step(2, 300)];
    let op_id = 1;
    let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

    let result = allocation_step_callback(result.new_state, true, 100, op_id).unwrap();
    let result = allocation_step_callback(result.new_state, true, 200, op_id).unwrap();
    let result = allocation_step_callback(result.new_state, true, 300, op_id).unwrap();

    // Should still be Allocating until complete_allocation is called
    assert!(matches!(result.new_state, OpState::Allocating(ref s) if s.remaining == 0));

    let result = complete_allocation(result.new_state, op_id, None).unwrap();
    assert!(matches!(result.new_state, OpState::Idle));
}

// Proptest Regression Edge Cases (deterministic)
// These tests encode specific edge cases discovered by proptest regressions.
// See proptest-regressions/transitions.txt and property_tests.proptest-regressions.

/// Regression: withdrawal with amount=1, escrow_shares=1 followed by
/// collected1=1 leaves remaining=0 — second step is correctly skipped.
/// Seeds: transitions.txt cc 0a7898a6, property_tests cc 0bd733bf.
#[test]
fn regression_withdrawal_amount_one_single_step() {
    let request = WithdrawalRequest {
        op_id: 1,
        request_id: 1,
        amount: 1,
        receiver: Address([34; 32]),
        owner: Address([17; 32]),
        escrow_shares: 1,
    };
    let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
    assert!(result.new_state.is_withdrawing());

    let w = result.new_state.as_withdrawing().unwrap();
    assert_eq!(w.remaining, 1);
    assert_eq!(w.collected, 0);

    // Collect 1 — remaining becomes 0, no second step possible.
    let step1 = withdrawal_step_callback(result.new_state, 1, 1).unwrap();
    let w1 = step1.new_state.as_withdrawing().unwrap();
    assert_eq!(w1.collected, 1);
    assert_eq!(w1.remaining, 0);
    assert_eq!(w1.index, 1);
}

/// Regression: withdrawal_collected with burn_shares <= escrow_shares succeeds
/// when amount=1 and we collected everything in one step.
/// Verifies the full withdrawal flow completes for minimal amounts.
#[test]
fn regression_minimal_withdrawal_full_flow() {
    let request = WithdrawalRequest {
        op_id: 1,
        request_id: 1,
        amount: 1,
        receiver: Address([34; 32]),
        owner: Address([17; 32]),
        escrow_shares: 1,
    };
    let result = start_withdrawal(OpState::Idle, request).unwrap();

    // Collect exactly 1
    let step = withdrawal_step_callback(result.new_state, 1, 1).unwrap();

    // Now we have remaining=0. Build the final state for withdrawal_collected.
    // burn_shares=1 which equals escrow_shares=1 — should succeed.
    let collected = withdrawal_collected(step.new_state, 1, 1);
    assert!(collected.is_ok());
    let final_result = collected.unwrap();
    // After collection, we should be in Payout state
    assert!(final_result.new_state.is_payout());
}

/// Regression: invariant check with idle=1, external=1, delta=1 — ensures
/// total_assets != idle+external when extra delta is added.
/// Seed: property_tests cc 22c3dbcf.
#[test]
fn regression_invariant_check_minimal_delta() {
    let idle = 1u128;
    let external = 1u128;
    let delta = 1u128;
    let total = idle.saturating_add(external).saturating_add(delta);
    let mut state = VaultState::new();
    state.total_assets = total; // 3
    state.total_shares = 0;
    state.idle_assets = idle; // 1
    state.external_assets = external; // 1
    state.fee_anchor = FeeAccrualAnchor::new(total, TimestampNs(0));
    // total_assets(3) != idle(1) + external(1) = invariant violation
    assert!(
        !state.check_invariant(),
        "should detect invariant violation: 3 != 1 + 1"
    );
}

// Cross-Executor Parity Tests
use core::mem;
// Both NEAR and Soroban executors call the same kernel `apply_action`. These
// tests verify kernel determinism and that the state-preparation patterns both
// executors use (decrement idle_assets before kernel, restore on abort, etc.)
// produce consistent results.

/// Parity: kernel is deterministic — identical inputs always produce identical
/// outputs, regardless of which executor invokes it.
#[test]
fn parity_kernel_deterministic_deposit() {
    let config = default_config();
    let state = default_state();
    let action = KernelAction::Deposit {
        owner: owner_addr(1),
        receiver: owner_addr(1),
        assets_in: 1_000_000,
        min_shares_out: 0,
        now_ns: TimestampNs(100),
    };

    let result_a =
        apply_action(state.clone(), &config, None, &self_addr(), action.clone()).unwrap();
    let result_b = apply_action(state, &config, None, &self_addr(), action).unwrap();

    assert_eq!(
        result_a.state, result_b.state,
        "kernel must be deterministic"
    );
    assert_eq!(
        result_a.effects, result_b.effects,
        "effects must be deterministic"
    );
}

/// Parity: deposit produces identical shares regardless of the amount already
/// in the vault, so long as the share ratio is the same. Both executors rely
/// on this for preview_deposit_shares accuracy.
#[test]
fn parity_deposit_shares_ratio_stable() {
    let config = default_config();

    // Deposit into empty vault
    let state = default_state();
    let r1 = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: owner_addr(1),
            assets_in: 10_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    // Second deposit into the vault with 1:1 ratio
    let r2 = apply_action(
        r1.state.clone(),
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(2),
            receiver: owner_addr(2),
            assets_in: 5_000,
            min_shares_out: 0,
            now_ns: TimestampNs(2),
        },
    )
    .unwrap();

    // At 1:1 ratio, 5000 assets should mint 5000 shares
    assert_eq!(r2.state.total_shares, 15_000);
    assert_eq!(r2.state.total_assets, 15_000);
    assert_eq!(r2.state.idle_assets, 15_000);

    // preview_deposit_shares must agree
    let preview = preview_deposit_shares(&r1.state, &config, 5_000);
    assert_eq!(preview, 5_000, "preview must match actual deposit");
}

/// Parity: the executor pattern of decrementing idle_assets before calling
/// kernel BeginAllocating, then using kernel's AbortAllocating with
/// restore_idle, produces balanced accounting.
///
/// The kernel handles idle_assets decrement in BeginAllocating.
/// Soroban calls start_allocation directly (bypasses apply_action) and
/// handles idle_assets itself. NEAR delegates to apply_action.
#[test]
fn parity_executor_idle_decrement_abort_roundtrip() {
    let config = default_config();
    let mut state = default_state();
    state.idle_assets = 10_000;
    state.total_assets = 10_000;

    let plan = vec![alloc_step(0, 3_000), alloc_step(1, 2_000)];
    // --- Kernel: BeginAllocating decrements idle_assets ---
    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan: plan.clone(),
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    let op_id = match &result.state.op_state {
        OpState::Allocating(s) => s.op_id,
        _ => panic!("expected Allocating"),
    };
    // Kernel decrements idle_assets by allocation total
    assert_eq!(result.state.idle_assets, 5_000);

    // --- Kernel: AbortAllocating restores the allocation amount ---
    let result = apply_action(
        result.state,
        &config,
        None,
        &self_addr(),
        KernelAction::AbortAllocating { op_id },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    // After kernel-decrement + kernel-restore, we should be back to 10_000
    assert_eq!(
        result.state.idle_assets, 10_000,
        "abort must restore idle_assets to original"
    );
}

/// Parity: kernel BeginAllocating decrements idle_assets, SyncExternalAssets
/// updates external_assets, FinishAllocating returns to Idle.
///
/// The kernel's 2x sanity guard on SyncExternalAssets means executors must sync
/// incrementally (after each market deposit), not all at once.
#[test]
fn parity_executor_full_allocation_cycle() {
    let config = default_config();
    let mut state = default_state();
    // Start with 80% idle, 20% already external — a realistic post-refresh state
    state.idle_assets = 8_000;
    state.external_assets = 2_000;
    state.total_assets = 10_000;
    state.fee_anchor = FeeAccrualAnchor::new(10_000, TimestampNs(0));

    let plan = vec![alloc_step(0, 2_000), alloc_step(1, 1_000)];

    // BeginAllocating — kernel decrements idle_assets by alloc_total (3_000)
    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::BeginAllocating {
            op_id: 1,
            plan,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();
    // idle=5000, external=2000, total=7000 (assets in-flight to markets)
    assert_eq!(result.state.idle_assets, 5_000);
    assert_eq!(result.state.total_assets, 7_000);

    // Sync after allocation: external grew from 2000 to 5000 (allocated 3000).
    // 2x check: new_total = 5000+5000 = 10000, old total=7000, 7000*2=14000. OK.
    let result = apply_action(
        result.state,
        &config,
        None,
        &self_addr(),
        KernelAction::SyncExternalAssets {
            new_external_assets: 5_000,
            op_id: 1,
            now_ns: TimestampNs(2),
        },
    )
    .unwrap();
    assert_eq!(result.state.external_assets, 5_000);
    assert_eq!(result.state.total_assets, 10_000); // idle(5000) + ext(5000)

    // FinishAllocating
    let result = apply_action(
        result.state,
        &config,
        None,
        &self_addr(),
        KernelAction::FinishAllocating {
            op_id: 1,
            now_ns: TimestampNs(3),
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    assert_eq!(
        result.state.idle_assets, 5_000,
        "idle = 8000 - 3000 allocated"
    );
    assert_eq!(
        result.state.external_assets, 5_000,
        "external = 2000 + 3000 allocated"
    );
    assert_eq!(result.state.total_assets, 10_000, "total unchanged");
}

/// Parity: deposit → request_withdraw → execute → stop → settle roundtrip.
/// Both executors rely on this flow producing balanced accounting.
/// Flow: Idle → Withdrawing → Payout → Idle.
#[test]
fn parity_deposit_withdraw_settle_roundtrip() {
    let config = default_config();
    let state = default_state();
    let vault = self_addr();
    let user = owner_addr(1);

    // Deposit 10_000
    let result = apply_action(
        state,
        &config,
        None,
        &vault,
        KernelAction::Deposit {
            owner: user,
            receiver: user,
            assets_in: 10_000,
            min_shares_out: 0,
            now_ns: TimestampNs(100),
        },
    )
    .unwrap();
    assert_eq!(result.state.total_shares, 10_000);

    // Request withdraw of all shares
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::RequestWithdraw {
            owner: user,
            receiver: user,
            shares: 10_000,
            min_assets_out: 0,
            now_ns: TimestampNs(200),
        },
    )
    .unwrap();
    assert!(!result.state.withdraw_queue.pending_withdrawals().is_empty());

    // ExecuteWithdraw: Idle → Withdrawing
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(300),
        },
    )
    .unwrap();
    assert!(result.state.op_state.is_withdrawing());
    let op_id = result.state.op_state.op_id().unwrap();

    // Advance withdrawal: collect full amount (simulates executor pulling from idle)
    let mut state = result.state;
    let tr = withdrawal_step_callback(mem::take(&mut state.op_state), op_id, 10_000).unwrap();
    state.op_state = tr.new_state;

    // Now remaining=0, use withdrawal_collected → Payout
    let tr = withdrawal_collected(mem::take(&mut state.op_state), op_id, 10_000).unwrap();
    state.op_state = tr.new_state;
    assert!(state.op_state.is_payout());

    // SettlePayout: Payout → Idle
    let result = apply_action(
        state,
        &config,
        None,
        &vault,
        KernelAction::SettlePayout {
            op_id,
            outcome: PayoutOutcome::Success,
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    assert_eq!(result.state.total_shares, 0, "shares burned");
    assert_eq!(
        result.state.total_assets, 0,
        "kernel decrements assets on payout"
    );
    assert_eq!(
        result.state.idle_assets, 0,
        "kernel decrements idle on payout"
    );
}

/// Parity: preview functions agree with actual kernel actions.
/// Both executors expose preview_deposit/preview_redeem views that must match
/// the kernel's actual share/asset calculations.
#[test]
fn parity_preview_matches_actual() {
    let config = default_config();
    let mut state = default_state();
    state.idle_assets = 50_000;
    state.total_assets = 50_000;
    state.total_shares = 25_000; // 2:1 asset:share ratio

    // Preview deposit
    let preview_shares = preview_deposit_shares(&state, &config, 10_000);

    // Actual deposit
    let result = apply_action(
        state.clone(),
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: owner_addr(1),
            assets_in: 10_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();
    let actual_shares = result.state.total_shares - 25_000;
    assert_eq!(
        preview_shares, actual_shares,
        "preview_deposit must match actual"
    );

    // Preview withdraw
    let preview_assets = preview_withdraw_assets(&state, &config, 5_000);

    let result = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::RequestWithdraw {
            owner: owner_addr(1),
            receiver: owner_addr(1),
            shares: 5_000,
            min_assets_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();
    let queued = result
        .state
        .withdraw_queue
        .pending_withdrawals()
        .values()
        .next()
        .unwrap();
    assert_eq!(
        preview_assets, queued.expected_assets,
        "preview_withdraw must match actual"
    );
}

/// Parity: refresh cycle with external growth updates share price identically
/// for both executors.
#[test]
fn parity_refresh_external_growth() {
    let config = default_config();
    let mut state = default_state();
    state.idle_assets = 5_000;
    state.external_assets = 5_000;
    state.total_assets = 10_000;
    state.total_shares = 10_000;
    state.fee_anchor = FeeAccrualAnchor::new(10_000, TimestampNs(0));

    let vault = self_addr();

    // BeginRefreshing
    let result = apply_action(
        state,
        &config,
        None,
        &vault,
        KernelAction::BeginRefreshing {
            op_id: 1,
            plan: vec![0, 1],
            now_ns: TimestampNs(100),
        },
    )
    .unwrap();

    // SyncExternalAssets with growth (5000 → 7000)
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::SyncExternalAssets {
            new_external_assets: 7_000,
            op_id: 1,
            now_ns: TimestampNs(200),
        },
    )
    .unwrap();

    // FinishRefreshing
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::FinishRefreshing {
            op_id: 1,
            now_ns: TimestampNs(300),
        },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    assert_eq!(result.state.external_assets, 7_000);
    assert_eq!(
        result.state.total_assets, 12_000,
        "idle(5000) + external(7000)"
    );
    assert_eq!(result.state.total_shares, 10_000, "shares unchanged");

    // Share price reflects external growth. With virtual_shares=0 and
    // virtual_assets=0, effective_totals adds +1 to both supply and assets,
    // so the exact preview uses floor(1000 * 12001 / 10001) = 1199.
    let preview = preview_withdraw_assets(&result.state, &config, 1_000);
    // Approximate check: growth is reflected (value > 1000)
    assert!(
        (1_199..=1_200).contains(&preview),
        "share price reflects growth, got {preview}"
    );
}

/// Parity: effect vectors are identical for deposit regardless of address
/// domain prefix (NEAR uses sha256(accountId), Soroban uses sha256(domain+strkey)).
/// The kernel doesn't care about address construction — only that the same
/// address produces the same effects.
#[test]
fn parity_effects_identical_for_deposit() {
    let config = default_config();
    let state = default_state();

    // Simulate "NEAR-style" address
    let near_addr: [u8; 32] = [0xAA; 32];
    let r1 = apply_action(
        state.clone(),
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: Address(near_addr),
            receiver: Address(near_addr),
            assets_in: 5_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    // Simulate "Soroban-style" address
    let soroban_addr: [u8; 32] = [0xBB; 32];
    let r2 = apply_action(
        state,
        &config,
        None,
        &self_addr(),
        KernelAction::Deposit {
            owner: Address(soroban_addr),
            receiver: Address(soroban_addr),
            assets_in: 5_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    // States must match (modulo addresses in effects)
    assert_eq!(r1.state.total_assets, r2.state.total_assets);
    assert_eq!(r1.state.total_shares, r2.state.total_shares);
    assert_eq!(r1.state.idle_assets, r2.state.idle_assets);
    assert_eq!(r1.effects.len(), r2.effects.len(), "same number of effects");
}

/// Parity: abort_withdrawing refunds shares identically for both executors.
#[test]
fn parity_abort_withdrawing_refund() {
    let config = default_config();
    let state = default_state();
    let vault = self_addr();
    let user = owner_addr(1);

    // Deposit
    let result = apply_action(
        state,
        &config,
        None,
        &vault,
        KernelAction::Deposit {
            owner: user,
            receiver: user,
            assets_in: 10_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    // Request withdrawal
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::RequestWithdraw {
            owner: user,
            receiver: user,
            shares: 5_000,
            min_assets_out: 0,
            now_ns: TimestampNs(2),
        },
    )
    .unwrap();
    let shares_before = result.state.total_shares;

    // Execute withdrawal — transitions Idle → Withdrawing
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::ExecuteWithdraw {
            now_ns: TimestampNs(3),
        },
    )
    .unwrap();
    let op_id = result.state.op_state.op_id().unwrap();

    // Abort with full refund
    let result = apply_action(
        result.state,
        &config,
        None,
        &vault,
        KernelAction::AbortWithdrawing { op_id },
    )
    .unwrap();

    assert!(result.state.op_state.is_idle());
    assert_eq!(
        result.state.total_shares, shares_before,
        "shares restored after abort"
    );
    assert_eq!(result.state.total_assets, 10_000, "assets unchanged");
}

/// Parity: multiple deposits from different users produce consistent share
/// ratios. Both executors must see the same share price for concurrent users.
#[test]
fn parity_concurrent_deposits_share_consistency() {
    let config = default_config();
    let state = default_state();
    let vault = self_addr();

    // User A deposits 10_000
    let r1 = apply_action(
        state,
        &config,
        None,
        &vault,
        KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: owner_addr(1),
            assets_in: 10_000,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        },
    )
    .unwrap();

    // User B deposits 20_000
    let r2 = apply_action(
        r1.state,
        &config,
        None,
        &vault,
        KernelAction::Deposit {
            owner: owner_addr(2),
            receiver: owner_addr(2),
            assets_in: 20_000,
            min_shares_out: 0,
            now_ns: TimestampNs(2),
        },
    )
    .unwrap();

    // User C deposits 5_000
    let r3 = apply_action(
        r2.state.clone(),
        &config,
        None,
        &vault,
        KernelAction::Deposit {
            owner: owner_addr(3),
            receiver: owner_addr(3),
            assets_in: 5_000,
            min_shares_out: 0,
            now_ns: TimestampNs(3),
        },
    )
    .unwrap();

    assert_eq!(r3.state.total_assets, 35_000);
    assert_eq!(r3.state.total_shares, 35_000); // 1:1 ratio maintained
    assert_eq!(r3.state.idle_assets, 35_000);

    // Preview at this state should also agree
    let preview = preview_deposit_shares(&r2.state, &config, 5_000);
    assert_eq!(preview, 5_000, "preview matches third deposit");
}

proptest! {
    /// Parity property: for any valid deposit amount, kernel produces
    /// identical state whether called once or reconstructed and called again.
    #[test]
    fn prop_parity_kernel_deposit_deterministic(
        assets in 1u128..=1_000_000_000u128,
        initial_idle in 0u128..=1_000_000_000u128,
        initial_shares in 0u128..=1_000_000_000u128,
    ) {
        let config = default_config();
        let state = VaultState::with_initial(
            initial_idle,
            initial_shares,
            initial_idle,
            0,
            TimestampNs(0),
        );
        let action = KernelAction::Deposit {
            owner: owner_addr(1),
            receiver: owner_addr(1),
            assets_in: assets,
            min_shares_out: 0,
            now_ns: TimestampNs(1),
        };

        let r1 = apply_action(state.clone(), &config, None, &self_addr(), action.clone());
        let r2 = apply_action(state, &config, None, &self_addr(), action);

        match (r1, r2) {
            (Ok(a), Ok(b)) => {
                prop_assert_eq!(a.state, b.state);
                prop_assert_eq!(a.effects, b.effects);
            }
            (Err(_), Err(_)) => {} // both fail = parity
            _ => prop_assert!(false, "one succeeded, the other failed"),
        }
    }

    /// Parity property: successful deposits mint exactly the previewed shares.
    #[test]
    fn prop_deposit_mints_previewed_shares(
        assets in 1u128..=1_000_000_000u128,
        initial_idle in 0u128..=1_000_000_000u128,
        initial_shares in 0u128..=1_000_000_000u128,
    ) {
        let config = default_config();
        let state = VaultState::with_initial(
            initial_idle,
            initial_shares,
            initial_idle,
            0,
            TimestampNs(0),
        );
        let previewed_shares = preview_deposit_shares(&state, &config, assets);
        if previewed_shares == 0 {
            return Ok(());
        }

        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::Deposit {
                owner: owner_addr(1),
                receiver: receiver_addr(1),
                assets_in: assets,
                min_shares_out: previewed_shares,
                now_ns: TimestampNs(1),
            },
        );
        prop_assert!(result.is_ok(), "deposit with previewed minimum should succeed");
        let result = result.expect("result was checked as ok");

        let minted_shares = result
            .effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { owner, shares } if *owner == receiver_addr(1) => {
                    Some(*shares)
                }
                _ => None,
            })
            .expect("successful deposit must mint shares");

        prop_assert_eq!(minted_shares, previewed_shares);
        prop_assert!(result.state.check_invariant());
    }

    /// Parity property: executor idle_assets decrement + kernel abort always
    /// restores to original value.
    #[test]
    fn prop_parity_executor_decrement_abort_roundtrip(
        idle in 1_000u128..=1_000_000_000u128,
        alloc_frac in 1u128..=100u128, // % of idle to allocate
    ) {
        let config = default_config();
        let alloc_amount = idle * alloc_frac / 100;
        if alloc_amount == 0 { return Ok(()); }

        let state = VaultState::with_initial(idle, 0, idle, 0, TimestampNs(0));

        // Kernel handles idle_assets decrement in BeginAllocating
        let result = apply_action(
            state,
            &config,
            None,
            &self_addr(),
            KernelAction::BeginAllocating {
                op_id: 1,
                plan: vec![alloc_step(0, alloc_amount)],
                now_ns: TimestampNs(1),
            },
        );
        prop_assert!(result.is_ok());

        let after_alloc = result.unwrap().state;
        prop_assert_eq!(after_alloc.idle_assets, idle - alloc_amount, "kernel must decrement idle_assets");

        let result = apply_action(
            after_alloc,
            &config,
            None,
            &self_addr(),
            KernelAction::AbortAllocating { op_id: 1 },
        );
        prop_assert!(result.is_ok());
        let final_state = result.unwrap().state;

        prop_assert_eq!(final_state.idle_assets, idle, "abort must restore original idle");
        prop_assert!(final_state.op_state.is_idle());
    }

    /// Parity property: deposit followed by full withdrawal request always
    /// queues the expected_assets equal to the deposited amount (at 1:1 ratio).
    #[test]
    fn prop_parity_deposit_withdraw_request_assets(
        amount in 1u128..=1_000_000_000u128,
    ) {
        let config = default_config();
        let state = default_state();
        let vault = self_addr();
        let user = owner_addr(1);

        let r = apply_action(
            state,
            &config,
            None,
            &vault,
            KernelAction::Deposit {
                owner: user,
                receiver: user,
                assets_in: amount,
                min_shares_out: 0,
                now_ns: TimestampNs(1),
            },
        ).unwrap();

        let r = apply_action(
            r.state,
            &config,
            None,
            &vault,
            KernelAction::RequestWithdraw {
                owner: user,
                receiver: user,
                shares: amount,
                min_assets_out: 0,
                now_ns: TimestampNs(2),
            },
        ).unwrap();

        prop_assert_eq!(r.state.withdraw_queue.pending_withdrawals().len(), 1);
        let queued = r
            .state
            .withdraw_queue
            .pending_withdrawals()
            .values()
            .next()
            .unwrap();
        prop_assert_eq!(queued.expected_assets, amount, "at 1:1 ratio, expected_assets == deposited");
    }
}

fn spec_addr(tag: u8, index: u64) -> [u8; 32] {
    let mut address = [0u8; 32];
    address[0] = tag;
    address[1..9].copy_from_slice(&index.to_le_bytes());
    address
}

fn spec_vault_addr() -> Address {
    Address(spec_addr(0xAA, 0))
}

proptest! {
    #[test]
    fn prop_spec_deposit_updates_state(assets in 1u64..1_000_000) {
        let state = VaultState::new();
        let config = default_config();
        let result = apply_action(
            state,
            &config,
            None,
            &spec_vault_addr(),
            KernelAction::Deposit {
                owner: Address(spec_addr(0x11, 1)),
                receiver: Address(spec_addr(0x22, 2)),
                assets_in: assets as u128,
                min_shares_out: 0,
                now_ns: TimestampNs(0),
            },
        )
        .unwrap();

        prop_assert_eq!(result.state.total_assets, assets as u128);
        prop_assert_eq!(result.state.idle_assets, assets as u128);
        prop_assert!(result.state.total_shares > 0);
        prop_assert!(result.state.check_invariant());
    }

    #[test]
    fn prop_spec_withdraw_queue_fifo(n in 1u8..20) {
        let mut queue = WithdrawQueue::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let id = queue
                .enqueue(
                    Address(spec_addr(0x33, i as u64)),
                    Address(spec_addr(0x44, i as u64)),
                    10,
                    10,
                    TimestampNs(i as u64),
                    1024,
                )
                .unwrap();
            ids.push(id);
        }

        for expected in ids {
            let (id, _) = queue.dequeue().unwrap();
            prop_assert_eq!(id, expected);
        }
        prop_assert_eq!(queue.len(), 0);
    }
}

#[cfg(feature = "action-sync-external")]
proptest! {
    #[test]
    fn prop_spec_sync_external_assets_updates_total(
        idle in 0u64..1_000_000,
        existing_external in 0u64..1_000_000,
        in_flight in 0u64..1_000_000,
        delta in 0u64..1_000_000,
    ) {
        let external = existing_external + delta;
        let mut state = VaultState::new();
        state.idle_assets = idle as u128;
        state.external_assets = existing_external as u128;
        state.total_assets = idle as u128 + existing_external as u128;
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 7,
            index: 0,
            remaining: in_flight as u128,
            plan: vec![alloc_step(0, in_flight as u128)],
        });

        let config = default_config();
        let result = apply_action(
            state,
            &config,
            None,
            &spec_vault_addr(),
            KernelAction::SyncExternalAssets {
                new_external_assets: external as u128,
                op_id: 7,
                now_ns: TimestampNs(0),
            },
        )
        .unwrap();

        prop_assert_eq!(result.state.external_assets, external as u128);
        prop_assert_eq!(result.state.total_assets, idle as u128 + external as u128);
        prop_assert!(result.state.check_invariant());
    }
}
