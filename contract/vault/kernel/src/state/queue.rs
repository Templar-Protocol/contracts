//! Chain-agnostic withdrawal queue types and pure logic functions.
//!
//! This module provides data structures for pending withdrawals and pure
//! functions for queue logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

use crate::math::number::Number;
use crate::math::wad::Wad;
use crate::types::{ActorId, EscrowSettlement, TimestampNs};

// ============================================================================
// Constants
// ============================================================================

/// Minimum withdrawal amount in base asset units to prevent dust.
/// Withdrawals below this threshold should be rejected.
pub const MIN_WITHDRAWAL_ASSETS: u128 = 1_000;

/// Maximum queue length before rejecting new requests.
/// This prevents unbounded queue growth and potential DoS vectors.
pub const MAX_QUEUE_LENGTH: u32 = 1_000;

/// Default cooldown period in nanoseconds (24 hours).
/// Withdrawals cannot be processed until this time has elapsed.
pub const DEFAULT_COOLDOWN_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

// ============================================================================
// Types
// ============================================================================

/// A pending withdrawal request in the queue.
///
/// Represents a user's request to redeem shares for underlying assets.
/// The shares are held in escrow until the withdrawal is processed.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingWithdrawal {
    /// Owner of the shares being redeemed.
    pub owner: ActorId,
    /// Receiver of the assets (may differ from owner).
    pub receiver: ActorId,
    /// Shares held in escrow awaiting redemption.
    pub escrow_shares: u128,
    /// Expected assets at time of request (for slippage checking).
    pub expected_assets: u128,
    /// Timestamp (nanoseconds) when the request was made.
    pub requested_at_ns: TimestampNs,
}

impl PendingWithdrawal {
    /// Create a new pending withdrawal request.
    #[inline]
    #[must_use]
    pub fn new(
        owner: ActorId,
        receiver: ActorId,
        escrow_shares: u128,
        expected_assets: u128,
        requested_at_ns: TimestampNs,
    ) -> Self {
        Self {
            owner,
            receiver,
            escrow_shares,
            expected_assets,
            requested_at_ns,
        }
    }

    /// Check if this withdrawal has passed the cooldown period.
    #[inline]
    #[must_use]
    pub fn is_past_cooldown(&self, now_ns: TimestampNs, cooldown_ns: u64) -> bool {
        now_ns >= self.requested_at_ns.saturating_add(cooldown_ns)
    }
}

/// Result of attempting to satisfy a withdrawal from available assets.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawalResult {
    /// Assets actually transferred to the receiver.
    pub assets_out: u128,
    /// Settlement describing how escrowed shares are handled.
    pub settlement: EscrowSettlement,
}

/// Status information for a single withdrawal request in the queue.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawalRequestStatus {
    /// Position in the queue (0 = head).
    pub index: u32,
    /// Sum of expected assets of requests ahead in the queue.
    pub depth_assets: u128,
    /// The withdrawal request details.
    pub withdrawal: PendingWithdrawal,
}

/// Aggregate status of the entire withdrawal queue.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueStatus {
    /// Number of pending withdrawal requests.
    pub length: u32,
    /// Total expected assets across all pending requests.
    pub total_expected_assets: u128,
    /// Total escrowed shares across all pending requests.
    pub total_escrow_shares: u128,
}

impl Default for QueueStatus {
    fn default() -> Self {
        Self {
            length: 0,
            total_expected_assets: 0,
            total_escrow_shares: 0,
        }
    }
}

// ============================================================================
// Pure Functions - Validation
// ============================================================================

/// Check if a withdrawal amount meets the minimum threshold.
///
/// Returns `true` if the assets are at or above `MIN_WITHDRAWAL_ASSETS`.
#[inline]
#[must_use]
pub fn is_valid_withdrawal_amount(assets: u128) -> bool {
    assets >= MIN_WITHDRAWAL_ASSETS
}

/// Check if the queue can accept a new withdrawal request.
///
/// Returns `true` if the current length is below `MAX_QUEUE_LENGTH`.
#[inline]
#[must_use]
pub fn can_enqueue(current_length: u32) -> bool {
    current_length < MAX_QUEUE_LENGTH
}

/// Check if a withdrawal request has passed its cooldown period.
///
/// # Arguments
/// * `requested_at_ns` - When the withdrawal was requested (nanoseconds).
/// * `now_ns` - Current timestamp (nanoseconds).
/// * `cooldown_ns` - Required cooldown period (nanoseconds).
#[inline]
#[must_use]
pub fn is_past_cooldown(requested_at_ns: TimestampNs, now_ns: TimestampNs, cooldown_ns: u64) -> bool {
    now_ns >= requested_at_ns.saturating_add(cooldown_ns)
}

// ============================================================================
// Pure Functions - Satisfaction Checks
// ============================================================================

/// Check if a withdrawal can be satisfied given available assets.
///
/// A withdrawal can be satisfied if the available assets meet or exceed
/// the expected asset amount from the withdrawal request.
///
/// # Arguments
/// * `withdrawal` - The pending withdrawal request.
/// * `available_assets` - Assets currently available for withdrawal.
#[inline]
#[must_use]
pub fn can_satisfy_withdrawal(withdrawal: &PendingWithdrawal, available_assets: u128) -> bool {
    available_assets >= withdrawal.expected_assets
}

/// Check if a withdrawal can be partially satisfied.
///
/// A partial satisfaction is possible when:
/// 1. Available assets are non-zero but less than expected.
/// 2. The available amount meets the minimum withdrawal threshold.
///
/// # Arguments
/// * `withdrawal` - The pending withdrawal request.
/// * `available_assets` - Assets currently available.
#[inline]
#[must_use]
pub fn can_partially_satisfy(withdrawal: &PendingWithdrawal, available_assets: u128) -> bool {
    available_assets > 0
        && available_assets < withdrawal.expected_assets
        && available_assets >= MIN_WITHDRAWAL_ASSETS
}

/// Calculate how many withdrawals can be fully satisfied from a queue.
///
/// Iterates through withdrawals in order, counting how many can be fully
/// satisfied before running out of available assets.
///
/// # Arguments
/// * `withdrawals` - Iterator over pending withdrawals (in queue order).
/// * `available_assets` - Total assets available for withdrawals.
///
/// # Returns
/// Tuple of (count of satisfiable withdrawals, total assets needed for those withdrawals).
#[must_use]
pub fn count_satisfiable<'a, I>(withdrawals: I, available_assets: u128) -> (u32, u128)
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut count = 0u32;
    let mut total_assets = 0u128;

    for withdrawal in withdrawals {
        let new_total = total_assets.saturating_add(withdrawal.expected_assets);
        if new_total > available_assets {
            break;
        }
        total_assets = new_total;
        count = count.saturating_add(1);
    }

    (count, total_assets)
}

// ============================================================================
// Pure Functions - Settlement Computation
// ============================================================================

/// Compute escrow settlement when completing a withdrawal.
///
/// Determines how many shares to burn vs refund based on actual redemption
/// versus the original expected amount.
///
/// # Arguments
/// * `escrow_shares` - Total shares held in escrow.
/// * `expected_assets` - Assets expected at time of request.
/// * `actual_assets` - Assets actually being redeemed.
///
/// # Returns
/// `EscrowSettlement` with shares to burn and shares to refund.
///
/// # Logic
/// - If actual >= expected: burn all shares (full redemption).
/// - If actual < expected: burn proportional shares, refund the rest.
/// - If actual == 0: refund all shares (cancellation).
#[must_use]
pub fn compute_settlement(
    escrow_shares: u128,
    expected_assets: u128,
    actual_assets: u128,
) -> EscrowSettlement {
    if escrow_shares == 0 {
        return EscrowSettlement {
            to_burn: 0,
            refund: 0,
        };
    }

    if actual_assets == 0 {
        // Full cancellation - refund all shares
        return EscrowSettlement::refund_all(escrow_shares);
    }

    if actual_assets >= expected_assets || expected_assets == 0 {
        // Full redemption - burn all shares
        return EscrowSettlement::burn_all(escrow_shares);
    }

    // Partial redemption - burn proportional shares, refund the rest
    // shares_to_burn = escrow_shares * actual_assets / expected_assets (floored)
    let shares_to_burn = Number::mul_div_floor(
        Number::from(escrow_shares),
        Number::from(actual_assets),
        Number::from(expected_assets),
    )
    .as_u128_trunc();

    let shares_to_refund = escrow_shares.saturating_sub(shares_to_burn);

    EscrowSettlement::partial(shares_to_burn, shares_to_refund)
}

/// Compute settlement using share price (WAD-scaled).
///
/// Alternative settlement computation using current share price instead of
/// asset ratios. Useful when share price is already computed.
///
/// # Arguments
/// * `escrow_shares` - Total shares held in escrow.
/// * `share_price_wad` - Current share price as a WAD (1e24 = 1.0).
/// * `original_share_price_wad` - Share price at time of request.
///
/// # Returns
/// `EscrowSettlement` based on price ratio.
#[must_use]
pub fn compute_settlement_by_price(
    escrow_shares: u128,
    share_price_wad: Wad,
    original_share_price_wad: Wad,
) -> EscrowSettlement {
    if escrow_shares == 0 || original_share_price_wad.is_zero() {
        return EscrowSettlement {
            to_burn: 0,
            refund: 0,
        };
    }

    // If current price >= original price, full burn
    if share_price_wad.0 >= original_share_price_wad.0 {
        return EscrowSettlement::burn_all(escrow_shares);
    }

    // Partial burn: ratio of current to original price
    // shares_to_burn = escrow_shares * current_price / original_price
    let shares_to_burn = Number::mul_div_floor(
        Number::from(escrow_shares),
        share_price_wad.0,
        original_share_price_wad.0,
    )
    .as_u128_trunc();

    let shares_to_refund = escrow_shares.saturating_sub(shares_to_burn);

    EscrowSettlement::partial(shares_to_burn, shares_to_refund)
}

/// Compute the withdrawal result for a fully satisfied withdrawal.
///
/// # Arguments
/// * `withdrawal` - The pending withdrawal to process.
/// * `available_assets` - Assets available (must be >= withdrawal.expected_assets).
///
/// # Returns
/// `Some(WithdrawalResult)` if withdrawal can be satisfied, `None` otherwise.
#[must_use]
pub fn compute_full_withdrawal(
    withdrawal: &PendingWithdrawal,
    available_assets: u128,
) -> Option<WithdrawalResult> {
    if !can_satisfy_withdrawal(withdrawal, available_assets) {
        return None;
    }

    Some(WithdrawalResult {
        assets_out: withdrawal.expected_assets,
        settlement: EscrowSettlement::burn_all(withdrawal.escrow_shares),
    })
}

/// Compute the withdrawal result for a partial withdrawal.
///
/// # Arguments
/// * `withdrawal` - The pending withdrawal to process.
/// * `available_assets` - Assets available (should be < withdrawal.expected_assets).
///
/// # Returns
/// `WithdrawalResult` with proportional shares burned.
#[must_use]
pub fn compute_partial_withdrawal(
    withdrawal: &PendingWithdrawal,
    available_assets: u128,
) -> WithdrawalResult {
    let actual_assets = available_assets.min(withdrawal.expected_assets);

    let settlement = compute_settlement(
        withdrawal.escrow_shares,
        withdrawal.expected_assets,
        actual_assets,
    );

    WithdrawalResult {
        assets_out: actual_assets,
        settlement,
    }
}

// ============================================================================
// Pure Functions - Queue Aggregation
// ============================================================================

/// Compute aggregate queue status from an iterator of withdrawals.
///
/// # Arguments
/// * `withdrawals` - Iterator over all pending withdrawals.
///
/// # Returns
/// `QueueStatus` with totals across all requests.
#[must_use]
pub fn compute_queue_status<'a, I>(withdrawals: I) -> QueueStatus
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut status = QueueStatus::default();

    for withdrawal in withdrawals {
        status.length = status.length.saturating_add(1);
        status.total_expected_assets = status
            .total_expected_assets
            .saturating_add(withdrawal.expected_assets);
        status.total_escrow_shares = status
            .total_escrow_shares
            .saturating_add(withdrawal.escrow_shares);
    }

    status
}

/// Find a withdrawal request's status by owner.
///
/// # Arguments
/// * `withdrawals` - Iterator over pending withdrawals in queue order.
/// * `owner` - The owner to search for.
///
/// # Returns
/// `Some(WithdrawalRequestStatus)` if found, `None` otherwise.
#[must_use]
pub fn find_request_status<'a, I>(
    withdrawals: I,
    owner: &ActorId,
) -> Option<WithdrawalRequestStatus>
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut index = 0u32;
    let mut depth_assets = 0u128;

    for withdrawal in withdrawals {
        if &withdrawal.owner == owner {
            return Some(WithdrawalRequestStatus {
                index,
                depth_assets,
                withdrawal: withdrawal.clone(),
            });
        }
        depth_assets = depth_assets.saturating_add(withdrawal.expected_assets);
        index = index.saturating_add(1);
    }

    None
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    fn make_withdrawal(owner: &str, shares: u128, expected: u128) -> PendingWithdrawal {
        PendingWithdrawal::new(
            owner.to_string(),
            owner.to_string(),
            shares,
            expected,
            1_000_000_000_000, // 1 second in ns
        )
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
        assert!(!is_past_cooldown(requested, requested, cooldown));
        assert!(!is_past_cooldown(requested, requested + cooldown - 1, cooldown));

        // Past cooldown
        assert!(is_past_cooldown(requested, requested + cooldown, cooldown));
        assert!(is_past_cooldown(requested, requested + cooldown + 1, cooldown));
    }

    #[test]
    fn test_can_satisfy_withdrawal() {
        let w = make_withdrawal("alice", 100, 1000);

        assert!(can_satisfy_withdrawal(&w, 1000));
        assert!(can_satisfy_withdrawal(&w, 2000));
        assert!(!can_satisfy_withdrawal(&w, 999));
        assert!(!can_satisfy_withdrawal(&w, 0));
    }

    #[test]
    fn test_can_partially_satisfy() {
        let w = make_withdrawal("alice", 100, 10_000);

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
            make_withdrawal("alice", 100, 1000),
            make_withdrawal("bob", 200, 2000),
            make_withdrawal("charlie", 300, 3000),
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

        // Zero expected (edge case, treated as full burn)
        let settlement = compute_settlement(100, 0, 500);
        assert_eq!(settlement.to_burn, 100);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_compute_full_withdrawal() {
        let w = make_withdrawal("alice", 100, 1000);

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
        let w = make_withdrawal("alice", 100, 1000);

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
    fn test_compute_queue_status() {
        let withdrawals: Vec<PendingWithdrawal> = vec![
            make_withdrawal("alice", 100, 1000),
            make_withdrawal("bob", 200, 2000),
            make_withdrawal("charlie", 300, 3000),
        ];

        let status = compute_queue_status(&withdrawals);
        assert_eq!(status.length, 3);
        assert_eq!(status.total_expected_assets, 6000);
        assert_eq!(status.total_escrow_shares, 600);
    }

    #[test]
    fn test_find_request_status() {
        let withdrawals: Vec<PendingWithdrawal> = vec![
            make_withdrawal("alice", 100, 1000),
            make_withdrawal("bob", 200, 2000),
            make_withdrawal("charlie", 300, 3000),
        ];

        // Find alice (first)
        let status = find_request_status(&withdrawals, &"alice".to_string());
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.index, 0);
        assert_eq!(status.depth_assets, 0);
        assert_eq!(status.withdrawal.escrow_shares, 100);

        // Find bob (second)
        let status = find_request_status(&withdrawals, &"bob".to_string());
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.index, 1);
        assert_eq!(status.depth_assets, 1000);

        // Find charlie (third)
        let status = find_request_status(&withdrawals, &"charlie".to_string());
        assert!(status.is_some());
        let status = status.unwrap();
        assert_eq!(status.index, 2);
        assert_eq!(status.depth_assets, 3000);

        // Not found
        let status = find_request_status(&withdrawals, &"dave".to_string());
        assert!(status.is_none());
    }

    #[test]
    fn test_pending_withdrawal_is_past_cooldown() {
        let w = PendingWithdrawal::new(
            "alice".to_string(),
            "alice".to_string(),
            100,
            1000,
            1_000_000_000_000, // 1 second
        );

        let cooldown = 60_000_000_000u64; // 60 seconds

        // Not past cooldown
        assert!(!w.is_past_cooldown(1_000_000_000_000, cooldown));
        assert!(!w.is_past_cooldown(1_059_999_999_999, cooldown));

        // Past cooldown
        assert!(w.is_past_cooldown(1_060_000_000_000, cooldown));
        assert!(w.is_past_cooldown(2_000_000_000_000, cooldown));
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
    }
}

// ============================================================================
// Property Tests
// ============================================================================

#[cfg(test)]
mod proptests {
    use super::*;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    /// Strategy for generating a PendingWithdrawal
    fn arb_withdrawal() -> impl Strategy<Value = PendingWithdrawal> {
        (
            1u32..1000u32,       // owner index
            1u128..=u64::MAX as u128,  // shares
            MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,  // expected_assets
            0u64..u64::MAX,      // timestamp
        )
            .prop_map(|(owner_idx, shares, expected, ts)| {
                PendingWithdrawal::new(
                    format!("owner_{}", owner_idx),
                    format!("owner_{}", owner_idx),
                    shares,
                    expected,
                    ts,
                )
            })
    }

    /// Strategy for generating a queue of withdrawals
    fn arb_queue(max_len: usize) -> impl Strategy<Value = Vec<PendingWithdrawal>> {
        proptest::collection::vec(arb_withdrawal(), 0..=max_len)
    }

    proptest! {
        // ===================================================================
        // Property: count_satisfiable is monotonic in available_assets
        // Invariant: If assets1 <= assets2 then count1 <= count2 and total1 <= total2
        // ===================================================================
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

        // ===================================================================
        // Property: count_satisfiable total <= available
        // Invariant: The total assets needed never exceeds available
        // ===================================================================
        #[test]
        fn count_satisfiable_total_bounded(
            queue in arb_queue(10),
            available in 0u128..=u64::MAX as u128,
        ) {
            let (_, total) = count_satisfiable(&queue, available);
            prop_assert!(total <= available, "total {} > available {}", total, available);
        }

        // ===================================================================
        // Property: count_satisfiable respects FIFO order
        // Invariant: If count = n, then queue[0..n] are exactly the satisfiable ones
        // ===================================================================
        #[test]
        fn count_satisfiable_respects_fifo(
            queue in arb_queue(10),
            available in 0u128..=u64::MAX as u128,
        ) {
            let (count, total) = count_satisfiable(&queue, available);

            // Sum of first `count` withdrawals should equal total
            let sum: u128 = queue.iter().take(count as usize).map(|w| w.expected_assets).sum();
            prop_assert_eq!(sum, total, "sum mismatch: {} vs {}", sum, total);

            // If there's a next item, adding it would exceed available
            if (count as usize) < queue.len() {
                let next = &queue[count as usize];
                prop_assert!(
                    total.saturating_add(next.expected_assets) > available,
                    "next item should not fit"
                );
            }
        }

        // ===================================================================
        // Property: compute_settlement burn + refund == escrow_shares
        // Invariant: Settlement conserves shares
        // ===================================================================
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

        // ===================================================================
        // Property: compute_settlement full burn when actual >= expected
        // Invariant: Burns all shares when redemption meets or exceeds expectation
        // ===================================================================
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

        // ===================================================================
        // Property: compute_settlement full refund when actual == 0
        // Invariant: Refunds all shares on cancellation
        // ===================================================================
        #[test]
        fn compute_settlement_full_refund_on_cancellation(
            escrow_shares in 1u128..=u64::MAX as u128,
            expected_assets in 1u128..=u64::MAX as u128,
        ) {
            let settlement = compute_settlement(escrow_shares, expected_assets, 0);

            prop_assert_eq!(settlement.to_burn, 0, "should burn none");
            prop_assert_eq!(settlement.refund, escrow_shares, "should refund all");
        }

        // ===================================================================
        // Property: compute_settlement proportional burn
        // Invariant: burn ratio approximately equals actual/expected ratio
        // ===================================================================
        #[test]
        fn compute_settlement_proportional(
            escrow_shares in 1u128..=1_000_000_000u128,
            expected_assets in 1u128..=1_000_000_000u128,
            actual_ratio_pct in 1u8..100u8,  // 1-99%
        ) {
            let actual_assets = (expected_assets as u128 * actual_ratio_pct as u128) / 100;
            if actual_assets == 0 || actual_assets >= expected_assets {
                return Ok(());  // Skip edge cases
            }

            let settlement = compute_settlement(escrow_shares, expected_assets, actual_assets);

            // Check proportionality (with tolerance for rounding)
            let expected_burn = (escrow_shares as u128 * actual_assets) / expected_assets;
            let diff = if settlement.to_burn > expected_burn {
                settlement.to_burn - expected_burn
            } else {
                expected_burn - settlement.to_burn
            };

            prop_assert!(diff <= 1, "burn not proportional: expected ~{}, got {}", expected_burn, settlement.to_burn);
        }

        // ===================================================================
        // Property: compute_settlement_by_price conserves shares
        // Invariant: burn + refund == escrow_shares
        // ===================================================================
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

        // ===================================================================
        // Property: compute_queue_status length matches
        // Invariant: status.length == queue.len()
        // ===================================================================
        #[test]
        fn compute_queue_status_length_correct(
            queue in arb_queue(20),
        ) {
            let status = compute_queue_status(&queue);
            prop_assert_eq!(status.length as usize, queue.len());
        }

        // ===================================================================
        // Property: compute_queue_status totals are sums
        // Invariant: totals equal manual sums
        // ===================================================================
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

        // ===================================================================
        // Property: find_request_status depth consistency
        // Invariant: If found, depth_assets = sum(queue[0..found_idx].expected_assets)
        // Note: find_request_status returns the FIRST occurrence of owner
        // ===================================================================
        #[test]
        fn find_request_status_depth_correct(
            queue in arb_queue(10),
        ) {
            if queue.is_empty() {
                return Ok(());
            }

            // Pick the first entry's owner to avoid duplicates issue
            let owner = &queue[0].owner;
            let status = find_request_status(&queue, owner);

            prop_assert!(status.is_some());
            let status = status.unwrap();

            // For the first occurrence, depth should be sum of expected_assets before its index
            let expected_depth: u128 = queue.iter().take(status.index as usize).map(|w| w.expected_assets).sum();
            prop_assert_eq!(status.depth_assets, expected_depth);
        }

        // ===================================================================
        // Property: is_valid_withdrawal_amount boundary
        // Invariant: valid iff amount >= MIN_WITHDRAWAL_ASSETS
        // ===================================================================
        #[test]
        fn is_valid_withdrawal_amount_boundary(
            amount in 0u128..=MIN_WITHDRAWAL_ASSETS * 2,
        ) {
            let valid = is_valid_withdrawal_amount(amount);
            prop_assert_eq!(valid, amount >= MIN_WITHDRAWAL_ASSETS);
        }

        // ===================================================================
        // Property: can_enqueue boundary
        // Invariant: can enqueue iff length < MAX_QUEUE_LENGTH
        // ===================================================================
        #[test]
        fn can_enqueue_boundary(
            length in 0u32..=MAX_QUEUE_LENGTH + 10,
        ) {
            let can = can_enqueue(length);
            prop_assert_eq!(can, length < MAX_QUEUE_LENGTH);
        }

        // ===================================================================
        // Property: is_past_cooldown consistency
        // Invariant: past cooldown iff now >= requested + cooldown
        // ===================================================================
        #[test]
        fn is_past_cooldown_consistency(
            requested_at in 0u64..=u64::MAX / 2,
            cooldown in 0u64..=u64::MAX / 4,
            delta in 0u64..=u64::MAX / 4,
        ) {
            let now = requested_at.saturating_add(delta);
            let threshold = requested_at.saturating_add(cooldown);
            let past = is_past_cooldown(requested_at, now, cooldown);
            prop_assert_eq!(past, now >= threshold);
        }

        // ===================================================================
        // Property: can_satisfy_withdrawal consistency
        // Invariant: can satisfy iff available >= expected
        // ===================================================================
        #[test]
        fn can_satisfy_withdrawal_consistency(
            expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
            available in 0u128..=u64::MAX as u128,
        ) {
            let w = PendingWithdrawal::new(
                "owner".to_string(),
                "receiver".to_string(),
                1000,
                expected,
                0,
            );
            let can = can_satisfy_withdrawal(&w, available);
            prop_assert_eq!(can, available >= expected);
        }

        // ===================================================================
        // Property: can_partially_satisfy consistency
        // Invariant: partial iff 0 < available < expected and available >= MIN
        // ===================================================================
        #[test]
        fn can_partially_satisfy_consistency(
            expected in MIN_WITHDRAWAL_ASSETS + 1..=u64::MAX as u128,
            available in 0u128..=u64::MAX as u128,
        ) {
            let w = PendingWithdrawal::new(
                "owner".to_string(),
                "receiver".to_string(),
                1000,
                expected,
                0,
            );
            let can = can_partially_satisfy(&w, available);
            let should = available > 0 && available < expected && available >= MIN_WITHDRAWAL_ASSETS;
            prop_assert_eq!(can, should);
        }

        // ===================================================================
        // Property: compute_full_withdrawal returns Some iff satisfiable
        // Invariant: Returns Some when can_satisfy_withdrawal is true
        // ===================================================================
        #[test]
        fn compute_full_withdrawal_consistency(
            shares in 1u128..=u64::MAX as u128,
            expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
            available in 0u128..=u64::MAX as u128,
        ) {
            let w = PendingWithdrawal::new(
                "owner".to_string(),
                "receiver".to_string(),
                shares,
                expected,
                0,
            );
            let result = compute_full_withdrawal(&w, available);
            let can = can_satisfy_withdrawal(&w, available);

            prop_assert_eq!(result.is_some(), can);
        }

        // ===================================================================
        // Property: compute_partial_withdrawal assets_out bounded
        // Invariant: assets_out <= min(available, expected)
        // ===================================================================
        #[test]
        fn compute_partial_withdrawal_bounded(
            shares in 1u128..=u64::MAX as u128,
            expected in MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128,
            available in 0u128..=u64::MAX as u128,
        ) {
            let w = PendingWithdrawal::new(
                "owner".to_string(),
                "receiver".to_string(),
                shares,
                expected,
                0,
            );
            let result = compute_partial_withdrawal(&w, available);

            prop_assert!(result.assets_out <= expected);
            prop_assert!(result.assets_out <= available);
        }
    }
}
