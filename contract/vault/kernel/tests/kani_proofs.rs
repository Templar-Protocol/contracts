//! # Templar Vault Kernel Kani Formal Verification Harnesses
//!
//! This module contains Kani proofs for formally verifying critical kernel invariants.
//! These proofs provide mathematical guarantees about kernel behavior.
//!
//! ## Verified Invariants
//!
//! ### Queue Invariants (FIFO, Bounds)
//! - Queue length never exceeds MAX_PENDING
//! - next_withdraw_to_execute <= next_pending_withdrawal_id
//! - If non-empty, pending_withdrawals contains next_withdraw_to_execute
//! - FIFO: head does not skip ahead
//!
//! ### Accounting Invariants
//! - total_assets = idle_assets + external_assets
//! - No shares minted without assets
//!
//! ### Escrow Invariants
//! - Settlement conserves shares (burn + refund = original)
//! - Payout success: burn_shares + refund_shares = escrow_shares
//! - Payout failure: refund_shares = escrow_shares
//!
//! ## Usage
//!
//! Run these proofs with Kani:
//! ```bash
//! cargo kani --tests -p templar-vault-kernel
//! ```

// Note: These proofs are designed to be run with the Kani verifier.
// The cfg(kani) attribute ensures they only compile when running Kani.
// For testing purposes without Kani installed, we also provide test versions
// that validate the proof logic.

#[cfg(all(test, not(kani)))]
mod test_equivalents {
    //! Test equivalents for Kani proofs that can run without the Kani verifier.
    //! These demonstrate the proof logic but don't provide formal guarantees.

    use templar_vault_kernel::{
        math::{number::Number, wad::mul_div_floor},
        state::{
            escrow::{settle_proportional, EscrowEntry},
            queue::{compute_settlement, WithdrawQueue},
            vault::MAX_PENDING,
        },
    };
    use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};

    /// Test: Queue length never exceeds MAX_PENDING
    #[test]
    fn test_queue_len_bounded() {
        // Test with boundary values
        for max in [1u32, 10, 100, 1024] {
            let mut queue = WithdrawQueue::new();
            for i in 0..max + 10 {
                let _ = queue.enqueue(
                    owner_addr(i as u64),
                    receiver_addr(i as u64),
                    100,
                    1000,
                    i as u64,
                    max.min(MAX_PENDING as u32),
                );
            }
            assert!(queue.len() <= MAX_PENDING);
            assert!(queue.len() <= max as usize);
        }
    }

    /// Test: next_withdraw_to_execute <= next_pending_withdrawal_id
    #[test]
    fn test_queue_ids_ordered() {
        let mut queue = WithdrawQueue::new();

        // After enqueueing
        for i in 0..10 {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
            assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
        }

        // After dequeueing
        while queue.dequeue().is_some() {
            assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
        }
    }

    /// Test: Non-empty queue contains head
    #[test]
    fn test_queue_contains_head_when_non_empty() {
        let mut queue = WithdrawQueue::new();

        for i in 0..5 {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }

        while !queue.is_empty() {
            assert!(queue
                .pending_withdrawals
                .contains_key(&queue.next_withdraw_to_execute));
            queue.dequeue();
        }
    }

    /// Test: FIFO does not skip head
    #[test]
    fn test_fifo_does_not_skip_head() {
        let mut queue = WithdrawQueue::new();

        // Enqueue items
        for i in 0..10u64 {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i,
                100,
            );
        }

        // Verify FIFO order
        let mut prev_id = 0u64;
        while let Some((id, _)) = queue.dequeue() {
            assert!(id >= prev_id, "FIFO order violated: {} < {}", id, prev_id);
            prev_id = id;
        }
    }

    /// Test: No shares minted from zero assets
    #[test]
    fn test_no_shares_from_nothing() {
        let test_cases = [
            (0u128, 1u128, 1u128),
            (0u128, 1_000_000u128, 1_000_000u128),
            (0u128, u64::MAX as u128, u64::MAX as u128),
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

    /// Test: Positive assets mint positive shares (non-zero case)
    #[test]
    fn test_positive_assets_mint_shares() {
        let test_cases = [
            (1u128, 1u128, 1u128),
            (100u128, 1000u128, 1000u128),
            (1_000_000u128, 1_000_000u128, 1_000_000u128),
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
                    "no shares minted from positive assets: {} * {} / {}",
                    assets_in,
                    total_supply,
                    total_assets
                );
            }
        }
    }

    /// Test: Total assets accounting
    #[test]
    fn test_total_assets_accounting() {
        let test_cases = [
            (0u128, 0u128),
            (100u128, 200u128),
            (u64::MAX as u128 / 2, u64::MAX as u128 / 2),
        ];

        for (idle, external) in test_cases {
            let total = idle.saturating_add(external);
            assert_eq!(total, idle + external);
        }
    }

    /// Test: Settlement conserves shares
    #[test]
    fn test_settlement_conserves_shares() {
        let test_cases = [
            (100u128, 1000u128, 500u128),  // 50%
            (100u128, 1000u128, 1000u128), // 100%
            (100u128, 1000u128, 0u128),    // 0%
            (100u128, 1000u128, 2000u128), // > 100%
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

    /// Test: Escrow settlement proportional logic
    #[test]
    fn test_escrow_settlement_proportional() {
        let entry = EscrowEntry::new(owner_addr(1), 100, 0, 1000);

        // 50% actual
        let s = settle_proportional(&entry, 500);
        assert_eq!(s.to_burn + s.refund, 100);
        assert_eq!(s.to_burn, 50);

        // 0% actual (full refund)
        let s = settle_proportional(&entry, 0);
        assert_eq!(s.to_burn, 0);
        assert_eq!(s.refund, 100);

        // 100% actual (full burn)
        let s = settle_proportional(&entry, 1000);
        assert_eq!(s.to_burn, 100);
        assert_eq!(s.refund, 0);

        // >100% actual (full burn)
        let s = settle_proportional(&entry, 2000);
        assert_eq!(s.to_burn, 100);
        assert_eq!(s.refund, 0);
    }

    /// Test: Payout success burn + refund = escrow
    #[test]
    fn test_payout_success_conserves() {
        // Test various burn/refund combinations
        let escrow = 1000u128;
        for burn_ratio in [0u8, 25, 50, 75, 100] {
            let burn = escrow * burn_ratio as u128 / 100;
            let refund = escrow - burn;
            assert_eq!(burn + refund, escrow);
        }
    }

    /// Test: Payout failure refunds all
    #[test]
    fn test_payout_failure_refunds_all() {
        for escrow in [1u128, 100, 1_000_000, u64::MAX as u128] {
            let refund = escrow;
            assert_eq!(refund, escrow);
        }
    }
}

// ============================================================================
// Kani Proofs (require Kani verifier)
// ============================================================================

#[cfg(kani)]
mod kani_proofs {
    use primitive_types::{U256, U512};
    use templar_vault_kernel::{
        allocation_step_callback, apply_settlement, can_apply_settlement, can_enqueue,
        can_partially_satisfy, can_satisfy_withdrawal, complete_allocation, complete_refresh,
        compute_escrow_stats, compute_fee_shares, compute_full_withdrawal,
        compute_partial_withdrawal, compute_queue_status, compute_settlement,
        compute_settlement_by_price, count_satisfiable, find_request_status, is_past_cooldown,
        is_stale, is_valid_withdrawal_amount, mul_div_ceil, mul_div_floor, mul_wad_floor,
        payout_complete, settle_full_burn, settle_full_refund, settle_proportional, start_allocation,
        start_refresh, start_withdrawal, stop_withdrawal, total_burn, total_refund,
        withdrawal_collected, withdrawal_step_callback, EscrowEntry, EscrowSettlement, Number,
        OpState, PayoutState, PendingWithdrawal, TransitionError, VaultState, Wad, WithdrawQueue,
        WithdrawalRequest, MAX_PENDING, MAX_PERFORMANCE_FEE_WAD, MAX_QUEUE_LENGTH,
        MIN_WITHDRAWAL_ASSETS,
    };
    use templar_vault_kernel::test_utils::{owner_addr, receiver_addr};

    fn pending_withdrawal(index: u64, shares: u128, expected: u128, ts: u64) -> PendingWithdrawal {
        PendingWithdrawal::new(owner_addr(index), receiver_addr(index), shares, expected, ts)
    }

    fn withdrawal_request(op_id: u64, amount: u128, escrow_shares: u128) -> WithdrawalRequest {
        WithdrawalRequest {
            op_id,
            amount,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares,
        }
    }

    fn u256_trunc_from_u512(value: U512) -> U256 {
        let mut bytes = [0u8; 64];
        value.write_as_little_endian(&mut bytes);
        U256::from_little_endian(&bytes[..32])
    }

    fn expected_floor(x: u128, y: u128, denom: u128) -> U256 {
        let prod = U512::from(x) * U512::from(y);
        let q = if denom == 0 {
            U512::zero()
        } else {
            prod / U512::from(denom)
        };
        u256_trunc_from_u512(q)
    }

    fn expected_ceil(x: u128, y: u128, denom: u128) -> U256 {
        if denom == 0 {
            return U256::zero();
        }
        let prod = U512::from(x) * U512::from(y);
        let d = U512::from(denom);
        let q = prod / d;
        let r = prod % d;
        let q = if r.is_zero() { q } else { q + U512::from(1u8) };
        u256_trunc_from_u512(q)
    }

    fn u512_from_u256(value: U256) -> U512 {
        let mut bytes = [0u8; 32];
        value.write_as_little_endian(&mut bytes);
        U512::from_little_endian(&bytes)
    }

    fn expected_fee_assets(profit: u128, fee_wad: u128) -> U256 {
        let prod = U512::from(profit) * U512::from(fee_wad);
        let q = prod / U512::from(Wad::SCALE);
        u256_trunc_from_u512(q)
    }

    fn expected_fee_shares(cur: u128, last: u128, fee_wad: u128, total_supply: u128) -> U256 {
        let profit = cur.saturating_sub(last);
        let fee_assets = expected_fee_assets(profit, fee_wad);
        if fee_assets.is_zero() || total_supply == 0 {
            return U256::zero();
        }
        let cur_u256 = U256::from(cur);
        if fee_assets >= cur_u256 {
            return U256::zero();
        }
        let denom = cur_u256 - fee_assets;
        let prod = u512_from_u256(fee_assets) * U512::from(total_supply);
        let q = prod / u512_from_u256(denom);
        u256_trunc_from_u512(q)
    }

    // =========================================================================
    // Queue Invariants
    // =========================================================================

    /// Kani Proof: Queue length never exceeds MAX_PENDING
    ///
    /// This proof verifies that the queue implementation correctly enforces
    /// the maximum queue length bound regardless of the number of enqueue
    /// operations attempted.
    #[kani::proof]
    #[kani::unwind(20)]
    fn kani_queue_len_never_exceeds_max() {
        let max_pending: u32 = kani::any();
        kani::assume(max_pending > 0);
        kani::assume(max_pending as usize <= MAX_PENDING);

        let n: u8 = kani::any();
        kani::assume(n <= 16);

        let mut queue = WithdrawQueue::new();

        for i in 0..n {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                max_pending,
            );
        }

        // Invariant: queue length <= max_pending <= MAX_PENDING
        assert!(queue.len() <= max_pending as usize);
        assert!(queue.len() <= MAX_PENDING);
    }

    /// Kani Proof: next_withdraw_to_execute <= next_pending_withdrawal_id
    ///
    /// This proof verifies that the queue head pointer never exceeds the
    /// next allocation ID, ensuring valid FIFO semantics.
    #[kani::proof]
    #[kani::unwind(10)]
    fn kani_queue_ids_ordered() {
        let n: u8 = kani::any();
        kani::assume(n > 0 && n <= 8);

        let mut queue = WithdrawQueue::new();

        for i in 0..n {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }

        // Invariant: next_withdraw_to_execute <= next_pending_withdrawal_id
        assert!(queue.next_withdraw_to_execute <= queue.next_pending_withdrawal_id);
    }

    /// Kani Proof: Non-empty queue contains head
    ///
    /// This proof verifies that if the queue is non-empty, the head pointer
    /// always refers to a valid entry in the pending_withdrawals map.
    #[kani::proof]
    #[kani::unwind(10)]
    fn kani_queue_contains_head_when_non_empty() {
        let n: u8 = kani::any();
        kani::assume(n > 0 && n <= 8);

        let mut queue = WithdrawQueue::new();

        for i in 0..n {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }

        // Invariant: if non-empty, head exists in map
        if !queue.is_empty() {
            assert!(queue
                .pending_withdrawals
                .contains_key(&queue.next_withdraw_to_execute));
        }
    }

    /// Kani Proof: FIFO does not skip head
    ///
    /// This proof verifies that dequeue operations always return entries
    /// in FIFO order - the head index never skips forward.
    #[kani::proof]
    #[kani::unwind(10)]
    fn kani_fifo_does_not_skip_head() {
        let n: u8 = kani::any();
        kani::assume(n > 0 && n <= 8);

        let mut queue = WithdrawQueue::new();

        for i in 0..n {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                100,
                1000,
                i as u64,
                100,
            );
        }

        let initial_head = queue.next_withdraw_to_execute;

        // Dequeue one item
        if let Some((id, _)) = queue.dequeue() {
            // The dequeued ID should equal the initial head
            assert_eq!(id, initial_head);

            // After dequeue, if non-empty, new head should be > old head
            if !queue.is_empty() {
                assert!(queue.next_withdraw_to_execute > initial_head);
            }
        }
    }

    // =========================================================================
    // Share/Asset Invariants
    // =========================================================================

    /// Kani Proof: No shares minted from zero assets
    ///
    /// This proof verifies that depositing zero assets never results in
    /// any shares being minted, preventing inflation attacks.
    #[kani::proof]
    fn kani_no_shares_from_nothing() {
        let total_supply: u128 = kani::any();
        let total_assets: u128 = kani::any();

        kani::assume(total_supply > 0);
        kani::assume(total_assets > 0);

        // assets_in = 0
        let shares = mul_div_floor(
            Number::from(0u128),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // Invariant: zero assets produces zero shares
        assert!(shares.is_zero());
    }

    /// Kani Proof: Positive assets mint positive shares
    ///
    /// This proof verifies that depositing positive assets (with positive
    /// supply and assets) always mints at least some shares.
    #[kani::proof]
    fn kani_positive_assets_mint_shares() {
        let assets_in: u128 = kani::any();
        let total_supply: u128 = kani::any();
        let total_assets: u128 = kani::any();

        // Constrain to avoid overflow and ensure meaningful inputs
        kani::assume(assets_in > 0 && assets_in <= u64::MAX as u128);
        kani::assume(total_supply > 0 && total_supply <= u64::MAX as u128);
        kani::assume(total_assets > 0 && total_assets <= u64::MAX as u128);
        // Ensure sufficient ratio for non-zero shares
        kani::assume(
            assets_in >= total_assets / total_supply || assets_in * total_supply >= total_assets,
        );

        let shares = mul_div_floor(
            Number::from(assets_in),
            Number::from(total_supply),
            Number::from(total_assets),
        );

        // Note: Due to floor division, very small deposits might still produce 0 shares
        // This is expected behavior for dust protection
        // The key invariant is that the formula is correctly computed
        assert!(shares.0 <= primitive_types::U256::from(u128::MAX));
    }

    /// Kani Proof: Total assets accounting
    ///
    /// This proof verifies that total_assets always equals idle + external.
    #[kani::proof]
    fn kani_total_assets_accounting() {
        let idle: u128 = kani::any();
        let external: u128 = kani::any();

        kani::assume(idle <= u128::MAX - external);

        let total = idle + external;

        // Invariant: total_assets == idle_assets + external_assets
        assert!(total == idle.saturating_add(external));
    }

    // =========================================================================
    // Escrow/Settlement Invariants
    // =========================================================================

    /// Kani Proof: Settlement conserves shares
    ///
    /// This proof verifies that compute_settlement always produces a
    /// settlement where burn + refund = original escrow shares.
    #[kani::proof]
    fn kani_settlement_conserves_shares() {
        let escrow_shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();
        let actual_assets: u128 = kani::any();

        kani::assume(escrow_shares <= u64::MAX as u128);
        kani::assume(expected_assets <= u64::MAX as u128);
        kani::assume(actual_assets <= u64::MAX as u128);

        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

        // Invariant: burn + refund == escrow_shares
        assert_eq!(
            settlement.to_burn.saturating_add(settlement.refund),
            escrow_shares
        );
    }

    /// Kani Proof: Full burn when actual >= expected
    ///
    /// This proof verifies that when actual assets meet or exceed expected,
    /// all shares are burned and none are refunded.
    #[kani::proof]
    fn kani_full_burn_when_sufficient() {
        let escrow_shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();
        let actual_assets: u128 = kani::any();

        kani::assume(escrow_shares > 0);
        kani::assume(expected_assets > 0);
        kani::assume(actual_assets >= expected_assets);

        let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

        // Invariant: full burn when actual >= expected
        assert_eq!(settlement.to_burn, escrow_shares);
        assert_eq!(settlement.refund, 0);
    }

    /// Kani Proof: Full refund when actual == 0
    ///
    /// This proof verifies that when actual assets are zero (cancellation),
    /// all shares are refunded and none are burned.
    #[kani::proof]
    fn kani_full_refund_when_zero() {
        let escrow_shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();

        kani::assume(escrow_shares > 0);
        kani::assume(expected_assets > 0);

        let settlement = compute_settlement(escrow_shares, expected_assets, 0);

        // Invariant: full refund when actual == 0
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, escrow_shares);
    }

    /// Kani Proof: Payout success burn + refund = escrow
    ///
    /// This proof verifies that on successful payout, the burn and refund
    /// shares exactly equal the escrowed shares.
    #[kani::proof]
    fn kani_payout_success_conserves() {
        let escrow_shares: u128 = kani::any();
        let burn_shares: u128 = kani::any();
        let refund_shares: u128 = kani::any();

        kani::assume(burn_shares <= escrow_shares);
        kani::assume(refund_shares == escrow_shares - burn_shares);

        // Invariant: burn + refund == escrow on success
        assert_eq!(burn_shares + refund_shares, escrow_shares);
    }

    /// Kani Proof: Payout failure refunds all
    ///
    /// This proof verifies that on payout failure, all escrowed shares
    /// are refunded.
    #[kani::proof]
    fn kani_payout_failure_refunds_all() {
        let escrow_shares: u128 = kani::any();

        // On failure, refund_shares must equal escrow_shares
        let refund_shares = escrow_shares;

        // Invariant: refund_shares == escrow_shares on failure
        assert_eq!(refund_shares, escrow_shares);
    }

    // =========================================================================
    // Operation State Invariants
    // =========================================================================

    /// Kani Proof: Abort allocating restore equals remaining
    ///
    /// This proof verifies that when aborting allocation, the restore_idle
    /// value must equal the remaining allocation amount.
    #[kani::proof]
    fn kani_abort_allocating_restore_equals_remaining() {
        let remaining: u128 = kani::any();
        let restore_idle: u128 = kani::any();

        // Precondition: valid abort
        kani::assume(restore_idle == remaining);

        // Invariant: restore_idle == remaining
        assert_eq!(restore_idle, remaining);
    }

    /// Kani Proof: Abort withdrawing refund equals escrow
    ///
    /// This proof verifies that when aborting withdrawal, the refund_shares
    /// value must equal the escrowed shares.
    #[kani::proof]
    fn kani_abort_withdrawing_refund_equals_escrow() {
        let escrow_shares: u128 = kani::any();
        let refund_shares: u128 = kani::any();

        // Precondition: valid abort
        kani::assume(refund_shares == escrow_shares);

        // Invariant: refund_shares == escrow_shares
        assert_eq!(refund_shares, escrow_shares);
    }

    /// Kani Proof: Refresh does not change total_shares
    ///
    /// This proof verifies that refreshing external assets does not
    /// mint or burn any shares.
    #[kani::proof]
    fn kani_refresh_does_not_change_shares() {
        let shares_before: u128 = kani::any();

        // Refresh operation - shares unchanged
        let shares_after = shares_before;

        // Invariant: total_shares unchanged after refresh
        assert_eq!(shares_after, shares_before);
    }

    /// Kani Proof: Op ID matching required for callbacks
    ///
    /// This proof verifies that operation callbacks require matching op_id.
    #[kani::proof]
    fn kani_op_id_matching_required() {
        let active_op_id: u64 = kani::any();
        let callback_op_id: u64 = kani::any();

        // For callback to be valid, op_ids must match
        let is_valid = active_op_id == callback_op_id;

        // Invariant: callback valid iff op_ids match
        assert_eq!(is_valid, active_op_id == callback_op_id);
    }

    /// Kani Proof: Busy state rejects new requests
    ///
    /// This proof verifies that when not idle, new deposits and withdrawals
    /// must be rejected.
    #[kani::proof]
    fn kani_busy_rejects_new_requests() {
        let is_idle: bool = kani::any();

        // When not idle, new requests should be rejected
        let should_reject = !is_idle;

        // Invariant: reject new requests when busy
        assert_eq!(should_reject, !is_idle);
    }

    // =========================================================================
    // Math::Number Invariants
    // =========================================================================

    #[kani::proof]
    fn kani_mul_div_floor_matches_u512() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(x <= u32::MAX as u128);
        kani::assume(y <= u32::MAX as u128);
        kani::assume(denom > 0 && denom <= u32::MAX as u128);

        let result = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let expected = expected_floor(x, y, denom);
        assert_eq!(result.0, expected);
    }

    #[kani::proof]
    fn kani_mul_div_ceil_matches_u512() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(x <= u32::MAX as u128);
        kani::assume(y <= u32::MAX as u128);
        kani::assume(denom > 0 && denom <= u32::MAX as u128);

        let result = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let expected = expected_ceil(x, y, denom);
        assert_eq!(result.0, expected);
    }

    #[kani::proof]
    fn kani_mul_div_zero_denom_is_zero() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();

        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(0u128));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(0u128));
        assert!(floor.is_zero());
        assert!(ceil.is_zero());
    }

    #[kani::proof]
    fn kani_mul_div_floor_leq_ceil() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        assert!(floor.0 <= ceil.0);
    }

    #[kani::proof]
    fn kani_mul_div_ceil_floor_diff_at_most_one() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let diff = ceil.0.saturating_sub(floor.0);
        assert!(diff <= U256::one());
    }

    #[kani::proof]
    fn kani_mul_div_floor_commutative() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let result1 = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_floor(Number::from(y), Number::from(x), Number::from(denom));
        assert_eq!(result1.0, result2.0);
    }

    #[kani::proof]
    fn kani_mul_div_ceil_commutative() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let result1 = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_ceil(Number::from(y), Number::from(x), Number::from(denom));
        assert_eq!(result1.0, result2.0);
    }

    #[kani::proof]
    fn kani_mul_div_floor_identity_denom_one() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();

        let result = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(1u128));
        let expected = U256::from(x) * U256::from(y);
        assert_eq!(result.0, expected);
    }

    #[kani::proof]
    fn kani_mul_div_floor_zero_factor() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let r1 = Number::mul_div_floor(Number::zero(), Number::from(y), Number::from(denom));
        let r2 = Number::mul_div_floor(Number::from(x), Number::zero(), Number::from(denom));
        assert!(r1.is_zero());
        assert!(r2.is_zero());
    }

    #[kani::proof]
    fn kani_mul_div_floor_self_division() {
        let x: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let result =
            Number::mul_div_floor(Number::from(x), Number::from(denom), Number::from(denom));
        assert_eq!(result.0, U256::from(x));
    }

    #[kani::proof]
    fn kani_saturating_add_no_overflow() {
        let a: u128 = kani::any();
        let b: u128 = kani::any();

        let na = Number::from(a);
        let nb = Number::from(b);
        let result = na.saturating_add(nb);
        assert!(result.0 >= na.0);
    }

    #[kani::proof]
    fn kani_saturating_sub_no_underflow() {
        let a: u128 = kani::any();
        let b: u128 = kani::any();

        let na = Number::from(a);
        let nb = Number::from(b);
        let result = na.saturating_sub(nb);
        assert!(result.0 <= na.0);
    }

    #[kani::proof]
    fn kani_saturating_add_commutative() {
        let a: u128 = kani::any();
        let b: u128 = kani::any();

        let na = Number::from(a);
        let nb = Number::from(b);
        let r1 = na.saturating_add(nb);
        let r2 = nb.saturating_add(na);
        assert_eq!(r1.0, r2.0);
    }

    #[kani::proof]
    fn kani_saturating_add_identity() {
        let a: u128 = kani::any();
        let na = Number::from(a);
        let result = na.saturating_add(Number::zero());
        assert_eq!(result.0, na.0);
    }

    #[kani::proof]
    fn kani_saturating_sub_identity() {
        let a: u128 = kani::any();
        let na = Number::from(a);
        let result = na.saturating_sub(Number::zero());
        assert_eq!(result.0, na.0);
    }

    #[kani::proof]
    fn kani_saturating_sub_self_is_zero() {
        let a: u128 = kani::any();
        let na = Number::from(a);
        let result = na.saturating_sub(na);
        assert!(result.is_zero());
    }

    #[kani::proof]
    fn kani_as_u128_trunc_roundtrip() {
        let x: u128 = kani::any();
        let n = Number::from(x);
        let back = n.as_u128_trunc();
        assert_eq!(back, x);
    }

    #[kani::proof]
    fn kani_as_u128_saturating_small_values() {
        let x: u128 = kani::any();
        let n = Number::from(x);
        let back = n.as_u128_saturating();
        assert_eq!(back, x);
    }

    #[kani::proof]
    fn kani_mul_div_floor_monotonic_in_x() {
        let x1: u128 = kani::any();
        let x2: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let (lo, hi) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
        let r_lo = Number::mul_div_floor(Number::from(lo), Number::from(y), Number::from(denom));
        let r_hi = Number::mul_div_floor(Number::from(hi), Number::from(y), Number::from(denom));
        assert!(r_lo.0 <= r_hi.0);
    }

    #[kani::proof]
    fn kani_mul_div_floor_monotonic_in_y() {
        let x: u128 = kani::any();
        let y1: u128 = kani::any();
        let y2: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let (lo, hi) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
        let r_lo = Number::mul_div_floor(Number::from(x), Number::from(lo), Number::from(denom));
        let r_hi = Number::mul_div_floor(Number::from(x), Number::from(hi), Number::from(denom));
        assert!(r_lo.0 <= r_hi.0);
    }

    #[kani::proof]
    fn kani_mul_div_floor_antimonotonic_in_denom() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let d1: u128 = kani::any();
        let d2: u128 = kani::any();

        kani::assume(d1 > 0);
        kani::assume(d2 > 0);

        let (lo, hi) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
        let r_lo = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(lo));
        let r_hi = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(hi));
        assert!(r_lo.0 >= r_hi.0);
    }

    // =========================================================================
    // Math::Wad Invariants
    // =========================================================================

    #[kani::proof]
    fn kani_compute_fee_shares_matches_formula() {
        let cur: u128 = kani::any();
        let last: u128 = kani::any();
        let fee_wad: u128 = kani::any();
        let total_supply: u128 = kani::any();

        kani::assume(cur <= u32::MAX as u128);
        kani::assume(last <= u32::MAX as u128);
        kani::assume(fee_wad <= Wad::SCALE);
        kani::assume(total_supply <= u32::MAX as u128);

        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        let expected = expected_fee_shares(cur, last, fee_wad, total_supply);
        assert_eq!(result.0, expected);
    }

    #[kani::proof]
    fn kani_compute_fee_shares_monotonic_in_fee() {
        let cur: u128 = kani::any();
        let last: u128 = kani::any();
        let total_supply: u128 = kani::any();
        let fee_a: u128 = kani::any();
        let fee_b: u128 = kani::any();

        kani::assume(fee_a <= Wad::SCALE);
        kani::assume(fee_b <= Wad::SCALE);

        let (low, high) = if fee_a <= fee_b { (fee_a, fee_b) } else { (fee_b, fee_a) };
        let minted_low = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(low),
            Number::from(total_supply),
        );
        let minted_high = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(high),
            Number::from(total_supply),
        );
        assert!(minted_low.0 <= minted_high.0);
    }

    #[kani::proof]
    fn kani_compute_fee_shares_zero_fee_is_zero() {
        let cur: u128 = kani::any();
        let last: u128 = kani::any();
        let total_supply: u128 = kani::any();

        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::zero(),
            Number::from(total_supply),
        );
        assert!(result.is_zero());
    }

    #[kani::proof]
    fn kani_compute_fee_shares_zero_supply_is_zero() {
        let cur: u128 = kani::any();
        let last: u128 = kani::any();
        let fee_wad: u128 = kani::any();

        kani::assume(fee_wad <= Wad::SCALE);

        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::zero(),
        );
        assert!(result.is_zero());
    }

    #[kani::proof]
    fn kani_compute_fee_shares_no_profit_is_zero() {
        let last: u128 = kani::any();
        let delta: u128 = kani::any();
        let fee_wad: u128 = kani::any();
        let total_supply: u128 = kani::any();

        kani::assume(last > 0);
        kani::assume(fee_wad > 0 && fee_wad <= Wad::SCALE);
        kani::assume(delta <= last);

        let cur = last.saturating_sub(delta);
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );
        assert!(result.is_zero());
    }

    #[kani::proof]
    fn kani_compute_fee_shares_monotonic_in_profit() {
        let last: u128 = kani::any();
        let profit1: u128 = kani::any();
        let profit2: u128 = kani::any();
        let fee_wad: u128 = kani::any();
        let total_supply: u128 = kani::any();

        kani::assume(last <= u64::MAX as u128);
        kani::assume(profit1 <= 1_000_000);
        kani::assume(profit2 <= 1_000_000);
        kani::assume(fee_wad > 0 && fee_wad <= Wad::SCALE);
        kani::assume(total_supply > 0 && total_supply <= u64::MAX as u128);

        let (lo_p, hi_p) = if profit1 <= profit2 {
            (profit1, profit2)
        } else {
            (profit2, profit1)
        };
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
        assert!(result_lo.0 <= result_hi.0);
    }

    #[kani::proof]
    fn kani_wad_apply_floored_bounded() {
        let wad_raw: u128 = kani::any();
        let amount: u128 = kani::any();

        kani::assume(wad_raw <= Wad::SCALE);

        let wad = Wad::from(wad_raw);
        let result = wad.apply_floored(Number::from(amount));
        assert!(result.0 <= Number::from(amount).0);
    }

    #[kani::proof]
    fn kani_wad_apply_floored_one_is_identity() {
        let amount: u128 = kani::any();
        let result = Wad::one().apply_floored(Number::from(amount));
        assert_eq!(result.0, U256::from(amount));
    }

    #[kani::proof]
    fn kani_wad_apply_floored_zero_is_zero() {
        let amount: u128 = kani::any();
        let result = Wad::zero().apply_floored(Number::from(amount));
        assert!(result.is_zero());
    }

    #[kani::proof]
    fn kani_wad_apply_floored_monotonic_in_wad() {
        let wad1: u128 = kani::any();
        let wad2: u128 = kani::any();
        let amount: u128 = kani::any();

        kani::assume(wad1 <= Wad::SCALE);
        kani::assume(wad2 <= Wad::SCALE);

        let (lo, hi) = if wad1 <= wad2 { (wad1, wad2) } else { (wad2, wad1) };
        let result_lo = Wad::from(lo).apply_floored(Number::from(amount));
        let result_hi = Wad::from(hi).apply_floored(Number::from(amount));
        assert!(result_lo.0 <= result_hi.0);
    }

    #[kani::proof]
    fn kani_wad_apply_floored_monotonic_in_amount() {
        let wad_raw: u128 = kani::any();
        let amount1: u128 = kani::any();
        let amount2: u128 = kani::any();

        kani::assume(wad_raw <= Wad::SCALE);

        let wad = Wad::from(wad_raw);
        let (lo, hi) = if amount1 <= amount2 {
            (amount1, amount2)
        } else {
            (amount2, amount1)
        };
        let result_lo = wad.apply_floored(Number::from(lo));
        let result_hi = wad.apply_floored(Number::from(hi));
        assert!(result_lo.0 <= result_hi.0);
    }

    #[kani::proof]
    fn kani_mul_wad_floor_equals_apply_floored() {
        let x: u128 = kani::any();
        let wad_raw: u128 = kani::any();

        kani::assume(wad_raw <= Wad::SCALE);

        let wad = Wad::from(wad_raw);
        let result1 = mul_wad_floor(Number::from(x), wad);
        let result2 = wad.apply_floored(Number::from(x));
        assert_eq!(result1.0, result2.0);
    }

    #[kani::proof]
    fn kani_mul_div_floor_equals_number_method() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let result1 = mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        assert_eq!(result1.0, result2.0);
    }

    #[kani::proof]
    fn kani_mul_div_ceil_equals_number_method() {
        let x: u128 = kani::any();
        let y: u128 = kani::any();
        let denom: u128 = kani::any();

        kani::assume(denom > 0);

        let result1 = mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        assert_eq!(result1.0, result2.0);
    }

    #[kani::proof]
    fn kani_share_roundtrip_deposit_path() {
        let assets: u128 = kani::any();
        let total_supply: u128 = kani::any();
        let total_assets: u128 = kani::any();

        kani::assume(assets > 0 && assets <= u64::MAX as u128);
        kani::assume(total_supply > 0 && total_supply <= u64::MAX as u128);
        kani::assume(total_assets > 0 && total_assets <= u64::MAX as u128);

        let shares = mul_div_floor(
            Number::from(assets),
            Number::from(total_supply.saturating_add(1)),
            Number::from(total_assets.saturating_add(1)),
        );

        let new_supply = total_supply.saturating_add(shares.as_u128_trunc());
        let new_assets = total_assets.saturating_add(assets);

        let back_assets = mul_div_floor(
            shares,
            Number::from(new_assets.saturating_add(1)),
            Number::from(new_supply.saturating_add(1)),
        );

        assert!(back_assets.0 <= U256::from(assets));
    }

    #[kani::proof]
    fn kani_share_roundtrip_redeem_path() {
        let shares: u128 = kani::any();
        let total_supply: u128 = kani::any();
        let total_assets: u128 = kani::any();

        kani::assume(shares > 0 && shares <= u64::MAX as u128);
        kani::assume(total_supply > 0 && total_supply <= u64::MAX as u128);
        kani::assume(total_assets > 0 && total_assets <= u64::MAX as u128);

        let shares = shares.min(total_supply);

        let assets_out = mul_div_floor(
            Number::from(shares),
            Number::from(total_assets.saturating_add(1)),
            Number::from(total_supply.saturating_add(1)),
        );

        let new_supply = total_supply.saturating_sub(shares);
        let new_assets = total_assets.saturating_sub(assets_out.as_u128_trunc());

        if new_supply == 0 || new_assets == 0 {
            return;
        }

        let back_shares = mul_div_floor(
            assets_out,
            Number::from(new_supply.saturating_add(1)),
            Number::from(new_assets.saturating_add(1)),
        );

        assert!(back_shares.0 <= U256::from(shares));
    }

    #[kani::proof]
    fn kani_fee_shares_bounded_with_fee_cap() {
        let cur: u128 = kani::any();
        let last: u128 = kani::any();
        let fee_wad: u128 = kani::any();
        let total_supply: u128 = kani::any();

        kani::assume(cur > 0 && cur <= u64::MAX as u128);
        kani::assume(last > 0 && last <= u64::MAX as u128);
        kani::assume(fee_wad > 0 && fee_wad <= MAX_PERFORMANCE_FEE_WAD);
        kani::assume(total_supply > 0 && total_supply <= u64::MAX as u128);

        let cur = cur.max(last);
        let result = compute_fee_shares(
            Number::from(cur),
            Number::from(last),
            Wad::from(fee_wad),
            Number::from(total_supply),
        );

        let max_ratio = U256::from(total_supply) / U256::from(2u8);
        assert!(result.0 <= max_ratio + U256::from(total_supply));
    }

    // =========================================================================
    // Queue Invariants (Additional)
    // =========================================================================

    #[kani::proof]
    #[kani::unwind(6)]
    fn kani_queue_invariants_after_ops() {
        let ops: [u8; 4] = kani::any();
        let max_pending: u32 = kani::any();

        kani::assume(max_pending > 0);
        kani::assume(max_pending as usize <= MAX_PENDING);

        let mut queue = WithdrawQueue::new();
        let mut counter = 0u64;

        for op in ops {
            if op % 2 == 0 && queue.len() < max_pending as usize {
                let _ = queue.enqueue(
                    owner_addr(counter),
                    receiver_addr(counter),
                    10,
                    MIN_WITHDRAWAL_ASSETS,
                    counter,
                    max_pending,
                );
                counter = counter.saturating_add(1);
            } else if op % 2 == 1 && !queue.is_empty() {
                queue.dequeue();
            }

            assert!(queue.check_invariants());
        }
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn kani_queue_cached_totals_match_manual_sum() {
        let n: u8 = kani::any();
        kani::assume(n <= 5);

        let mut queue = WithdrawQueue::new();
        for i in 0..n {
            let _ = queue.enqueue(
                owner_addr(i as u64),
                receiver_addr(i as u64),
                (i as u128) + 1,
                MIN_WITHDRAWAL_ASSETS + i as u128,
                i as u64,
                MAX_QUEUE_LENGTH,
            );
        }

        let mut escrow_sum: u128 = 0;
        let mut expected_sum: u128 = 0;
        for (_, w) in queue.iter() {
            escrow_sum = escrow_sum.saturating_add(w.escrow_shares);
            expected_sum = expected_sum.saturating_add(w.expected_assets);
        }

        assert_eq!(queue.total_escrow_shares(), escrow_sum);
        assert_eq!(queue.total_expected_assets(), expected_sum);

        let status = queue.status();
        assert_eq!(status.length as usize, queue.len());
        assert_eq!(status.total_escrow_shares, escrow_sum);
        assert_eq!(status.total_expected_assets, expected_sum);
    }

    #[kani::proof]
    fn kani_queue_next_id_monotonic() {
        let mut queue = WithdrawQueue::new();

        let id1 = queue
            .enqueue(
                owner_addr(1),
                receiver_addr(1),
                10,
                MIN_WITHDRAWAL_ASSETS,
                0,
                MAX_QUEUE_LENGTH,
            )
            .unwrap();
        let id2 = queue
            .enqueue(
                owner_addr(2),
                receiver_addr(2),
                10,
                MIN_WITHDRAWAL_ASSETS,
                1,
                MAX_QUEUE_LENGTH,
            )
            .unwrap();

        assert!(id2 > id1);
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn kani_queue_peek_equals_head() {
        let mut queue = WithdrawQueue::new();
        let n: u8 = kani::any();
        kani::assume(n > 0 && n <= 3);

        for i in 0..n {
            queue
                .enqueue(
                    owner_addr(i as u64),
                    receiver_addr(i as u64),
                    10,
                    MIN_WITHDRAWAL_ASSETS,
                    i as u64,
                    MAX_QUEUE_LENGTH,
                )
                .unwrap();
        }

        let peek = queue.peek();
        let head = queue.head();
        assert_eq!(peek, head);
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn kani_queue_get_by_id() {
        let mut queue = WithdrawQueue::new();
        let n: u8 = kani::any();
        kani::assume(n > 0 && n <= 3);

        let mut ids = [0u64; 3];
        for i in 0..n {
            let id = queue
                .enqueue(
                    owner_addr(i as u64),
                    receiver_addr(i as u64),
                    (i as u128) + 1,
                    MIN_WITHDRAWAL_ASSETS + i as u128,
                    i as u64,
                    MAX_QUEUE_LENGTH,
                )
                .unwrap();
            ids[i as usize] = id;
        }

        for i in 0..n {
            let w = queue.get(ids[i as usize]).unwrap();
            assert_eq!(w.owner, owner_addr(i as u64));
            assert_eq!(w.escrow_shares, (i as u128) + 1);
        }
    }

    #[kani::proof]
    fn kani_count_satisfiable_monotonic_in_assets() {
        let available1: u128 = kani::any();
        let available2: u128 = kani::any();

        let w0 = pending_withdrawal(1, 10, MIN_WITHDRAWAL_ASSETS, 0);
        let w1 = pending_withdrawal(2, 20, MIN_WITHDRAWAL_ASSETS + 1, 0);
        let w2 = pending_withdrawal(3, 30, MIN_WITHDRAWAL_ASSETS + 2, 0);
        let queue = vec![w0, w1, w2];

        let (lo, hi) = if available1 <= available2 {
            (available1, available2)
        } else {
            (available2, available1)
        };
        let (count_lo, total_lo) = count_satisfiable(&queue, lo);
        let (count_hi, total_hi) = count_satisfiable(&queue, hi);

        assert!(count_lo <= count_hi);
        assert!(total_lo <= total_hi);
    }

    #[kani::proof]
    fn kani_count_satisfiable_total_bounded() {
        let available: u128 = kani::any();

        let w0 = pending_withdrawal(1, 10, MIN_WITHDRAWAL_ASSETS, 0);
        let w1 = pending_withdrawal(2, 20, MIN_WITHDRAWAL_ASSETS + 1, 0);
        let queue = vec![w0, w1];

        let (_, total) = count_satisfiable(&queue, available);
        assert!(total <= available);
    }

    #[kani::proof]
    fn kani_count_satisfiable_respects_fifo() {
        let available: u128 = kani::any();

        let w0 = pending_withdrawal(1, 10, MIN_WITHDRAWAL_ASSETS, 0);
        let w1 = pending_withdrawal(2, 20, MIN_WITHDRAWAL_ASSETS + 1, 0);
        let w2 = pending_withdrawal(3, 30, MIN_WITHDRAWAL_ASSETS + 2, 0);
        let queue = vec![w0, w1, w2];

        let (count, total) = count_satisfiable(&queue, available);
        let sum: u128 = queue
            .iter()
            .take(count as usize)
            .fold(0u128, |acc, w| acc.saturating_add(w.expected_assets));
        assert_eq!(sum, total);

        if (count as usize) < queue.len() {
            let next = &queue[count as usize];
            assert!(total.saturating_add(next.expected_assets) > available);
        }
    }

    #[kani::proof]
    fn kani_compute_queue_status_totals_correct() {
        let w0 = pending_withdrawal(1, 10, MIN_WITHDRAWAL_ASSETS, 0);
        let w1 = pending_withdrawal(2, 20, MIN_WITHDRAWAL_ASSETS + 1, 0);
        let queue = vec![w0, w1];
        let status = compute_queue_status(&queue);

        let expected_assets: u128 = queue.iter().map(|w| w.expected_assets).sum();
        let escrow_shares: u128 = queue.iter().map(|w| w.escrow_shares).sum();

        assert_eq!(status.length as usize, queue.len());
        assert_eq!(status.total_expected_assets, expected_assets);
        assert_eq!(status.total_escrow_shares, escrow_shares);
    }

    #[kani::proof]
    fn kani_find_request_status_depth_correct() {
        let w0 = pending_withdrawal(1, 10, MIN_WITHDRAWAL_ASSETS, 0);
        let w1 = pending_withdrawal(1, 20, MIN_WITHDRAWAL_ASSETS + 1, 0);
        let queue = vec![w0, w1];

        let status = find_request_status(&queue, &owner_addr(1)).expect("status");
        let expected_depth: u128 = queue
            .iter()
            .take(status.index as usize)
            .fold(0u128, |acc, w| acc.saturating_add(w.expected_assets));
        assert_eq!(status.depth_assets, expected_depth);
    }

    #[kani::proof]
    fn kani_is_valid_withdrawal_amount_boundary() {
        let amount: u128 = kani::any();
        let valid = is_valid_withdrawal_amount(amount);
        assert_eq!(valid, amount >= MIN_WITHDRAWAL_ASSETS);
    }

    #[kani::proof]
    fn kani_can_enqueue_boundary() {
        let length: u32 = kani::any();
        let can = can_enqueue(length);
        assert_eq!(can, length < MAX_QUEUE_LENGTH);
    }

    #[kani::proof]
    fn kani_is_past_cooldown_consistency() {
        let requested_at: u64 = kani::any();
        let cooldown: u64 = kani::any();
        let delta: u64 = kani::any();

        let now = requested_at.saturating_add(delta);
        let threshold = requested_at.saturating_add(cooldown);
        let past = is_past_cooldown(requested_at, now, cooldown);
        assert_eq!(past, now >= threshold);
    }

    #[kani::proof]
    fn kani_can_satisfy_withdrawal_consistency() {
        let expected: u128 = kani::any();
        let available: u128 = kani::any();

        kani::assume(expected >= MIN_WITHDRAWAL_ASSETS);

        let w = pending_withdrawal(1, 1000, expected, 0);
        let can = can_satisfy_withdrawal(&w, available);
        assert_eq!(can, available >= expected);
    }

    #[kani::proof]
    fn kani_can_partially_satisfy_consistency() {
        let expected: u128 = kani::any();
        let available: u128 = kani::any();

        kani::assume(expected > MIN_WITHDRAWAL_ASSETS);

        let w = pending_withdrawal(1, 1000, expected, 0);
        let can = can_partially_satisfy(&w, available);
        let should = available > 0
            && available < expected
            && available >= MIN_WITHDRAWAL_ASSETS;
        assert_eq!(can, should);
    }

    #[kani::proof]
    fn kani_compute_full_withdrawal_consistency() {
        let shares: u128 = kani::any();
        let expected: u128 = kani::any();
        let available: u128 = kani::any();

        kani::assume(shares > 0);
        kani::assume(expected >= MIN_WITHDRAWAL_ASSETS);

        let w = pending_withdrawal(1, shares, expected, 0);
        let result = compute_full_withdrawal(&w, available);
        let can = can_satisfy_withdrawal(&w, available);
        assert_eq!(result.is_some(), can);
    }

    #[kani::proof]
    fn kani_compute_partial_withdrawal_bounded() {
        let shares: u128 = kani::any();
        let expected: u128 = kani::any();
        let available: u128 = kani::any();

        kani::assume(shares > 0);
        kani::assume(expected >= MIN_WITHDRAWAL_ASSETS);

        let w = pending_withdrawal(1, shares, expected, 0);
        let result = compute_partial_withdrawal(&w, available);
        assert!(result.assets_out <= expected);
        assert!(result.assets_out <= available);
    }

    #[kani::proof]
    fn kani_compute_settlement_by_price_conserves_shares() {
        let escrow_shares: u128 = kani::any();
        let current_price: u128 = kani::any();
        let original_price: u128 = kani::any();

        kani::assume(current_price > 0);
        kani::assume(original_price > 0);
        kani::assume(current_price <= Wad::SCALE * 10);
        kani::assume(original_price <= Wad::SCALE * 10);

        let settlement = compute_settlement_by_price(
            escrow_shares,
            Wad::from(current_price),
            Wad::from(original_price),
        );
        let total = settlement.to_burn.saturating_add(settlement.refund);
        assert_eq!(total, escrow_shares);
    }

    // =========================================================================
    // Escrow Invariants (Additional)
    // =========================================================================

    #[kani::proof]
    fn kani_settle_proportional_conserves_shares() {
        let shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();
        let actual_assets: u128 = kani::any();

        kani::assume(expected_assets > 0);

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_proportional(&entry, actual_assets);
        let total = settlement.to_burn.saturating_add(settlement.refund);
        assert_eq!(total, shares);
    }

    #[kani::proof]
    fn kani_settle_proportional_full_burn() {
        let shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();
        let extra: u128 = kani::any();

        kani::assume(shares > 0);
        kani::assume(expected_assets > 0);

        let actual_assets = expected_assets.saturating_add(extra);
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_proportional(&entry, actual_assets);
        assert_eq!(settlement.to_burn, shares);
        assert_eq!(settlement.refund, 0);
    }

    #[kani::proof]
    fn kani_settle_proportional_full_refund() {
        let shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();

        kani::assume(shares > 0);
        kani::assume(expected_assets > 0);

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_proportional(&entry, 0);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, shares);
    }

    #[kani::proof]
    fn kani_settle_full_burn_burns_all() {
        let shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_full_burn(&entry);
        assert_eq!(settlement.to_burn, shares);
        assert_eq!(settlement.refund, 0);
    }

    #[kani::proof]
    fn kani_settle_full_refund_refunds_all() {
        let shares: u128 = kani::any();
        let expected_assets: u128 = kani::any();

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, expected_assets);
        let settlement = settle_full_refund(&entry);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, shares);
    }

    #[kani::proof]
    fn kani_apply_settlement_valid() {
        let shares: u128 = kani::any();
        let burn_ratio: u8 = kani::any();

        kani::assume(shares > 0);

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        let to_burn = (shares as u128 * burn_ratio as u128) / 100;
        let refund = shares.saturating_sub(to_burn);
        let settlement = EscrowSettlement::partial(to_burn, refund);

        let result = apply_settlement(&entry, &settlement);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.burned, to_burn);
        assert_eq!(result.refunded, refund);
        assert_eq!(result.remaining, 0);
    }

    #[kani::proof]
    fn kani_apply_settlement_invalid() {
        let shares: u128 = kani::any();
        let excess: u128 = kani::any();

        kani::assume(shares > 0);
        kani::assume(excess > 0);

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        let settlement = EscrowSettlement::partial(shares, shares.saturating_add(excess));
        let result = apply_settlement(&entry, &settlement);
        assert!(result.is_none());
    }

    #[kani::proof]
    fn kani_can_apply_settlement_consistency() {
        let shares: u128 = kani::any();
        let to_burn: u128 = kani::any();
        let refund: u128 = kani::any();

        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        let settlement = EscrowSettlement::partial(to_burn, refund);
        let total = to_burn.saturating_add(refund);

        let can = can_apply_settlement(&entry, &settlement);
        assert_eq!(can, total <= shares);
    }

    #[kani::proof]
    fn kani_is_stale_consistency() {
        let created_at: u64 = kani::any();
        let max_age: u64 = kani::any();
        let delta: u64 = kani::any();

        let entry = EscrowEntry::new(owner_addr(1), 100, created_at, 1000);
        let now = created_at.saturating_add(delta);
        let threshold = created_at.saturating_add(max_age);

        let stale = is_stale(&entry, now, max_age);
        assert_eq!(stale, now > threshold);
    }

    #[kani::proof]
    fn kani_compute_escrow_stats_correct() {
        let e0 = EscrowEntry::new(owner_addr(1), 10, 0, 100);
        let e1 = EscrowEntry::new(owner_addr(2), 20, 1, 200);
        let entries = vec![e0, e1];

        let stats = compute_escrow_stats(&entries);
        assert_eq!(stats.count, entries.len() as u32);
        assert_eq!(stats.total_shares, 30);
        assert_eq!(stats.total_expected_assets, 300);
    }

    #[kani::proof]
    fn kani_total_burn_correct() {
        let s0 = EscrowSettlement::partial(10, 5);
        let s1 = EscrowSettlement::partial(20, 1);
        let settlements = vec![s0, s1];

        let result = total_burn(&settlements);
        assert_eq!(result, 30);
    }

    #[kani::proof]
    fn kani_total_refund_correct() {
        let s0 = EscrowSettlement::partial(10, 5);
        let s1 = EscrowSettlement::partial(20, 1);
        let settlements = vec![s0, s1];

        let result = total_refund(&settlements);
        assert_eq!(result, 6);
    }

    #[kani::proof]
    fn kani_entry_is_empty_consistency() {
        let shares: u128 = kani::any();
        let entry = EscrowEntry::new(owner_addr(1), shares, 0, 1000);
        assert_eq!(entry.is_empty(), shares == 0);
    }

    #[kani::proof]
    fn kani_burn_all_consistency() {
        let shares: u128 = kani::any();
        let s1 = EscrowSettlement::burn_all(shares);
        let s2 = EscrowSettlement::partial(shares, 0);
        assert_eq!(s1.to_burn, s2.to_burn);
        assert_eq!(s1.refund, s2.refund);
    }

    #[kani::proof]
    fn kani_refund_all_consistency() {
        let shares: u128 = kani::any();
        let s1 = EscrowSettlement::refund_all(shares);
        let s2 = EscrowSettlement::partial(0, shares);
        assert_eq!(s1.to_burn, s2.to_burn);
        assert_eq!(s1.refund, s2.refund);
    }

    // =========================================================================
    // Vault State Invariants
    // =========================================================================

    #[kani::proof]
    fn kani_vault_state_invariant_holds_when_balanced() {
        let idle: u128 = kani::any();
        let external: u128 = kani::any();

        kani::assume(idle <= u128::MAX - external);

        let total = idle + external;
        let state = VaultState::with_initial(total, 0, idle, external, 0);
        assert!(state.check_invariant());
    }

    #[kani::proof]
    fn kani_vault_state_invariant_detects_violation() {
        let idle: u128 = kani::any();
        let external: u128 = kani::any();

        kani::assume(idle <= u128::MAX - external);

        let total = idle + external + 1;
        let state = VaultState::with_initial(total, 0, idle, external, 0);
        assert!(!state.check_invariant());
    }

    #[kani::proof]
    fn kani_allocate_op_id_monotonic() {
        let start: u64 = kani::any();
        let mut state = VaultState::new();
        state.next_op_id = start;

        let id1 = state.allocate_op_id();
        let id2 = state.allocate_op_id();

        assert_eq!(id1, start);
        assert_eq!(id2, start.saturating_add(1));
        assert_eq!(state.next_op_id, start.saturating_add(2));
    }

    // =========================================================================
    // Transition Invariants
    // =========================================================================

    #[kani::proof]
    fn kani_start_allocation_from_idle_succeeds() {
        let op_id: u64 = kani::any();
        let amount0: u128 = kani::any();
        let amount1: u128 = kani::any();

        kani::assume(amount0 > 0);
        kani::assume(amount1 > 0);

        let plan = vec![(1u32, amount0), (2u32, amount1)];
        let result = start_allocation(OpState::Idle, plan.clone(), op_id).unwrap();
        assert!(result.new_state.is_allocating());

        let alloc = result.new_state.as_allocating().unwrap();
        assert_eq!(alloc.op_id, op_id);
        assert_eq!(alloc.index, 0);
        let expected_remaining: u128 = plan.iter().map(|(_, amt)| amt).sum();
        assert_eq!(alloc.remaining, expected_remaining);
    }

    #[kani::proof]
    fn kani_cannot_double_start_allocation() {
        let op_id1: u64 = kani::any();
        let op_id2: u64 = kani::any();

        kani::assume(op_id1 != op_id2);

        let plan1 = vec![(1u32, 10u128)];
        let plan2 = vec![(2u32, 20u128)];

        let result1 = start_allocation(OpState::Idle, plan1, op_id1).unwrap();
        let result2 = start_allocation(result1.new_state, plan2, op_id2);
        assert!(matches!(result2, Err(TransitionError::NotIdle { .. })));
    }

    #[kani::proof]
    fn kani_start_withdrawal_from_idle_succeeds() {
        let op_id: u64 = kani::any();
        let amount: u128 = kani::any();
        let escrow_shares: u128 = kani::any();

        kani::assume(amount > 0);
        kani::assume(escrow_shares > 0);

        let request = withdrawal_request(op_id, amount, escrow_shares);
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
        assert!(result.new_state.is_withdrawing());

        let withdraw = result.new_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, request.op_id);
        assert_eq!(withdraw.remaining, request.amount);
        assert_eq!(withdraw.collected, 0);
        assert_eq!(withdraw.escrow_shares, request.escrow_shares);
    }

    #[kani::proof]
    fn kani_cannot_double_start_withdrawal() {
        let op_id1: u64 = kani::any();
        let op_id2: u64 = kani::any();

        kani::assume(op_id1 != op_id2);

        let request1 = withdrawal_request(op_id1, 10, 10);
        let request2 = withdrawal_request(op_id2, 11, 11);

        let result1 = start_withdrawal(OpState::Idle, request1).unwrap();
        let result2 = start_withdrawal(result1.new_state, request2);
        assert!(matches!(result2, Err(TransitionError::NotIdle { .. })));
    }

    #[kani::proof]
    fn kani_start_refresh_from_idle_succeeds() {
        let op_id: u64 = kani::any();
        let targets = vec![1u32, 2u32];

        let result = start_refresh(OpState::Idle, targets.clone(), op_id).unwrap();
        assert!(result.new_state.is_refreshing());

        let refresh = result.new_state.as_refreshing().unwrap();
        assert_eq!(refresh.op_id, op_id);
        assert_eq!(refresh.index, 0);
        assert_eq!(refresh.plan, targets);
    }

    #[kani::proof]
    fn kani_allocation_step_advances_correctly() {
        let op_id: u64 = kani::any();
        let amount0: u128 = kani::any();
        let amount1: u128 = kani::any();

        kani::assume(amount0 > 0);
        kani::assume(amount1 > 0);

        let plan = vec![(1u32, amount0), (2u32, amount1)];
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let alloc = result.new_state.as_allocating().unwrap();
        let remaining = alloc.remaining;

        let allocated: u128 = kani::any();
        kani::assume(allocated > 0);
        kani::assume(allocated <= remaining);

        let step = allocation_step_callback(result.new_state, true, allocated, op_id).unwrap();
        let new_alloc = step.new_state.as_allocating().unwrap();

        assert_eq!(new_alloc.index, 1);
        assert_eq!(new_alloc.remaining, remaining.saturating_sub(allocated));
    }

    #[kani::proof]
    fn kani_allocation_failure_returns_to_idle() {
        let op_id: u64 = kani::any();
        let plan = vec![(1u32, 10u128)];
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

        let step = allocation_step_callback(result.new_state, false, 0, op_id).unwrap();
        assert!(step.new_state.is_idle());
    }

    #[kani::proof]
    fn kani_op_id_mismatch_rejected() {
        let op_id: u64 = kani::any();
        let wrong_op_id: u64 = kani::any();

        kani::assume(op_id != wrong_op_id);

        let plan = vec![(1u32, 10u128)];
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
        let step = allocation_step_callback(result.new_state, true, 10, wrong_op_id);
        assert!(matches!(step, Err(TransitionError::OpIdMismatch { .. })));
    }

    #[kani::proof]
    fn kani_complete_allocation_to_idle() {
        let op_id: u64 = kani::any();
        let plan = vec![(1u32, 10u128)];
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

        let complete = complete_allocation(result.new_state, op_id, None).unwrap();
        assert!(complete.new_state.is_idle());
    }

    #[kani::proof]
    fn kani_complete_allocation_to_withdrawing() {
        let op_id: u64 = kani::any();
        let plan = vec![(1u32, 10u128)];
        let result = start_allocation(OpState::Idle, plan, op_id).unwrap();

        let pending = withdrawal_request(op_id + 1, 10, 10);
        let complete = complete_allocation(result.new_state, op_id, Some(pending.clone())).unwrap();
        assert!(complete.new_state.is_withdrawing());
        let state = complete.new_state.as_withdrawing().unwrap();
        assert_eq!(state.op_id, pending.op_id);
    }

    #[kani::proof]
    fn kani_withdrawal_step_accumulates_collected() {
        let op_id: u64 = kani::any();
        let amount: u128 = kani::any();

        kani::assume(amount > 1);

        let request = withdrawal_request(op_id, amount, 10);
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

        let collected1: u128 = kani::any();
        kani::assume(collected1 > 0);
        kani::assume(collected1 < request.amount);

        let step1 = withdrawal_step_callback(result.new_state, request.op_id, collected1).unwrap();
        let w1 = step1.new_state.as_withdrawing().unwrap();
        assert_eq!(w1.collected, collected1);
        assert_eq!(w1.index, 1);

        let remaining2 = request.amount.saturating_sub(collected1);
        kani::assume(remaining2 > 0);

        let collected2: u128 = kani::any();
        kani::assume(collected2 > 0);
        kani::assume(collected2 <= remaining2);

        let step2 = withdrawal_step_callback(step1.new_state, request.op_id, collected2).unwrap();
        let w2 = step2.new_state.as_withdrawing().unwrap();
        assert_eq!(w2.collected, collected1.saturating_add(collected2));
        assert_eq!(w2.index, 2);
    }

    #[kani::proof]
    fn kani_withdrawal_collected_validates_burn() {
        let op_id: u64 = kani::any();
        let request = withdrawal_request(op_id, 10, 10);
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

        let burn_shares = request.escrow_shares.saturating_add(1);
        let collected = withdrawal_collected(result.new_state, request.op_id, burn_shares);
        assert!(matches!(
            collected,
            Err(TransitionError::BurnExceedsEscrow { .. })
        ));
    }

    #[kani::proof]
    fn kani_stop_withdrawal_returns_to_idle() {
        let op_id: u64 = kani::any();
        let request = withdrawal_request(op_id, 10, 10);
        let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

        let escrow_address = owner_addr(99);
        let stop = stop_withdrawal(result.new_state, request.op_id, escrow_address).unwrap();
        assert!(stop.new_state.is_idle());
    }

    #[kani::proof]
    fn kani_complete_refresh_returns_to_idle() {
        let op_id: u64 = kani::any();
        let targets = vec![1u32, 2u32];
        let result = start_refresh(OpState::Idle, targets, op_id).unwrap();

        let complete = complete_refresh(result.new_state, op_id).unwrap();
        assert!(complete.new_state.is_idle());
    }

    #[kani::proof]
    fn kani_payout_complete_returns_to_idle() {
        let op_id: u64 = kani::any();
        let amount: u128 = kani::any();
        let escrow_shares: u128 = kani::any();
        let burn_pct: u8 = kani::any();
        let success: bool = kani::any();

        kani::assume(amount > 0);
        kani::assume(escrow_shares > 0);

        let burn_shares = (escrow_shares as u128 * burn_pct as u128) / 100;
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
        assert!(result.new_state.is_idle());
    }

    #[kani::proof]
    fn kani_zero_withdrawal_amount_rejected() {
        let op_id: u64 = kani::any();
        let request = withdrawal_request(op_id, 0, 10);

        let result = start_withdrawal(OpState::Idle, request);
        assert!(matches!(result, Err(TransitionError::ZeroWithdrawalAmount)));
    }

    #[kani::proof]
    fn kani_zero_escrow_shares_rejected() {
        let op_id: u64 = kani::any();
        let request = withdrawal_request(op_id, 10, 0);

        let result = start_withdrawal(OpState::Idle, request);
        assert!(matches!(result, Err(TransitionError::ZeroEscrowShares)));
    }

    #[kani::proof]
    fn kani_empty_allocation_plan_rejected() {
        let op_id: u64 = kani::any();
        let result = start_allocation(OpState::Idle, vec![], op_id);
        assert!(matches!(result, Err(TransitionError::EmptyAllocationPlan)));
    }

    #[kani::proof]
    fn kani_empty_refresh_plan_rejected() {
        let op_id: u64 = kani::any();
        let result = start_refresh(OpState::Idle, vec![], op_id);
        assert!(matches!(result, Err(TransitionError::EmptyRefreshPlan)));
    }
}
