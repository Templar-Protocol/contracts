//! Chain-agnostic withdrawal queue types and pure logic functions.
//!
//! This module provides data structures for pending withdrawals and pure
//! functions for queue logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::math::number::Number;
use crate::math::wad::Wad;
use crate::types::{Address, EscrowSettlement, TimestampNs};

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
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingWithdrawal {
    /// Owner of the shares being redeemed.
    pub owner: Address,
    /// Receiver of the assets (may differ from owner).
    pub receiver: Address,
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
        owner: Address,
        receiver: Address,
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
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawalResult {
    /// Assets actually transferred to the receiver.
    pub assets_out: u128,
    /// Settlement describing how escrowed shares are handled.
    pub settlement: EscrowSettlement,
}

/// Status information for a single withdrawal request in the queue.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
pub fn is_past_cooldown(
    requested_at_ns: TimestampNs,
    now_ns: TimestampNs,
    cooldown_ns: u64,
) -> bool {
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
    owner: &Address,
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
// Queue Storage Types
// ============================================================================

use alloc::collections::BTreeMap;

// Re-export MAX_PENDING from vault module for convenience
pub use crate::state::vault::MAX_PENDING;

/// Withdrawal queue storage with FIFO ordering.
///
/// Maintains pending withdrawals keyed by monotonic IDs with escrow parity.
/// The queue uses a BTreeMap for efficient iteration and lookup, with two
/// pointers to track the FIFO head and next ID to allocate.
///
/// # Invariants
///
/// - `pending_withdrawals.len() <= max_pending_withdrawals <= MAX_PENDING`
/// - `next_withdraw_to_execute <= next_pending_withdrawal_id`
/// - If `pending_withdrawals.len() > 0`, then `pending_withdrawals` contains `next_withdraw_to_execute`
/// - FIFO withdrawal ordering; no skipping head
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawQueue {
    /// Pending withdrawals keyed by monotonic ID.
    pub pending_withdrawals: BTreeMap<u64, PendingWithdrawal>,
    /// ID of the next withdrawal to execute (queue head).
    pub next_withdraw_to_execute: u64,
    /// Next ID to allocate for new withdrawals (monotonic, never decremented).
    pub next_pending_withdrawal_id: u64,
}

impl Default for WithdrawQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WithdrawQueue {
    /// Create a new empty withdrawal queue.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending_withdrawals: BTreeMap::new(),
            next_withdraw_to_execute: 0,
            next_pending_withdrawal_id: 0,
        }
    }

    /// Create a queue with initial state (for testing or recovery).
    #[inline]
    #[must_use]
    pub fn with_state(
        pending_withdrawals: BTreeMap<u64, PendingWithdrawal>,
        next_withdraw_to_execute: u64,
        next_pending_withdrawal_id: u64,
    ) -> Self {
        Self {
            pending_withdrawals,
            next_withdraw_to_execute,
            next_pending_withdrawal_id,
        }
    }

    /// Returns the current queue length.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending_withdrawals.len()
    }

    /// Returns true if the queue is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending_withdrawals.is_empty()
    }

    /// Check if the queue can accept a new withdrawal given the max limit.
    ///
    /// # Arguments
    /// * `max_pending` - Maximum allowed pending withdrawals.
    ///
    /// # Returns
    /// `true` if the queue has room for another withdrawal.
    #[inline]
    #[must_use]
    pub fn can_enqueue(&self, max_pending: u32) -> bool {
        self.pending_withdrawals.len() < (max_pending as usize).min(MAX_PENDING)
    }

    /// Enqueue a new pending withdrawal.
    ///
    /// Allocates a new monotonic ID and inserts the withdrawal at the tail.
    ///
    /// # Arguments
    /// * `owner` - Owner of the shares being redeemed.
    /// * `receiver` - Receiver of the assets.
    /// * `escrow_shares` - Shares held in escrow.
    /// * `expected_assets` - Expected assets at time of request.
    /// * `requested_at_ns` - Timestamp of the request.
    /// * `max_pending` - Maximum allowed pending withdrawals.
    ///
    /// # Returns
    /// `Ok(id)` with the allocated withdrawal ID, or `Err(QueueError)` if full.
    pub fn enqueue(
        &mut self,
        owner: Address,
        receiver: Address,
        escrow_shares: u128,
        expected_assets: u128,
        requested_at_ns: TimestampNs,
        max_pending: u32,
    ) -> Result<u64, QueueError> {
        if !self.can_enqueue(max_pending) {
            return Err(QueueError::QueueFull {
                current: self.pending_withdrawals.len() as u32,
                max: max_pending,
            });
        }

        let id = self.next_pending_withdrawal_id;
        let withdrawal = PendingWithdrawal::new(
            owner,
            receiver,
            escrow_shares,
            expected_assets,
            requested_at_ns,
        );

        self.pending_withdrawals.insert(id, withdrawal);
        self.next_pending_withdrawal_id = self.next_pending_withdrawal_id.saturating_add(1);

        Ok(id)
    }

    /// Enqueue a pre-constructed pending withdrawal.
    ///
    /// Allocates a new monotonic ID and inserts the withdrawal at the tail.
    ///
    /// # Arguments
    /// * `withdrawal` - The pending withdrawal to enqueue.
    /// * `max_pending` - Maximum allowed pending withdrawals.
    ///
    /// # Returns
    /// `Ok(id)` with the allocated withdrawal ID, or `Err(QueueError)` if full.
    pub fn enqueue_withdrawal(
        &mut self,
        withdrawal: PendingWithdrawal,
        max_pending: u32,
    ) -> Result<u64, QueueError> {
        if !self.can_enqueue(max_pending) {
            return Err(QueueError::QueueFull {
                current: self.pending_withdrawals.len() as u32,
                max: max_pending,
            });
        }

        let id = self.next_pending_withdrawal_id;
        self.pending_withdrawals.insert(id, withdrawal);
        self.next_pending_withdrawal_id = self.next_pending_withdrawal_id.saturating_add(1);

        Ok(id)
    }

    /// Peek at the head of the queue without removing it.
    ///
    /// # Returns
    /// `Some((id, &withdrawal))` if non-empty, `None` if empty.
    #[must_use]
    pub fn peek(&self) -> Option<(u64, &PendingWithdrawal)> {
        if self.is_empty() {
            return None;
        }
        self.pending_withdrawals
            .get(&self.next_withdraw_to_execute)
            .map(|w| (self.next_withdraw_to_execute, w))
    }

    /// Get the head of the queue (same as peek).
    ///
    /// # Returns
    /// `Some((id, &withdrawal))` if non-empty, `None` if empty.
    #[inline]
    #[must_use]
    pub fn head(&self) -> Option<(u64, &PendingWithdrawal)> {
        self.peek()
    }

    /// Dequeue and return the head of the queue (FIFO).
    ///
    /// Removes the head and advances `next_withdraw_to_execute` to the next
    /// available ID in the queue (or to `next_pending_withdrawal_id` if empty).
    ///
    /// # Returns
    /// `Some((id, withdrawal))` if non-empty, `None` if empty.
    pub fn dequeue(&mut self) -> Option<(u64, PendingWithdrawal)> {
        if self.is_empty() {
            return None;
        }

        let head_id = self.next_withdraw_to_execute;
        let withdrawal = self.pending_withdrawals.remove(&head_id)?;

        // Advance to the next ID in the queue
        self.next_withdraw_to_execute = self
            .pending_withdrawals
            .keys()
            .next()
            .copied()
            .unwrap_or(self.next_pending_withdrawal_id);

        Some((head_id, withdrawal))
    }

    /// Get a pending withdrawal by ID.
    ///
    /// # Arguments
    /// * `id` - The withdrawal ID to look up.
    ///
    /// # Returns
    /// `Some(&withdrawal)` if found, `None` otherwise.
    #[inline]
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&PendingWithdrawal> {
        self.pending_withdrawals.get(&id)
    }

    /// Get a mutable reference to a pending withdrawal by ID.
    ///
    /// # Arguments
    /// * `id` - The withdrawal ID to look up.
    ///
    /// # Returns
    /// `Some(&mut withdrawal)` if found, `None` otherwise.
    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, id: u64) -> Option<&mut PendingWithdrawal> {
        self.pending_withdrawals.get_mut(&id)
    }

    /// Check if a withdrawal ID exists in the queue.
    ///
    /// # Arguments
    /// * `id` - The withdrawal ID to check.
    ///
    /// # Returns
    /// `true` if the withdrawal exists.
    #[inline]
    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.pending_withdrawals.contains_key(&id)
    }

    /// Iterate over all pending withdrawals in FIFO order.
    ///
    /// # Returns
    /// Iterator yielding `(id, &withdrawal)` pairs in order.
    #[must_use]
    pub fn iter(&self) -> impl Iterator<Item = (u64, &PendingWithdrawal)> {
        self.pending_withdrawals.iter().map(|(&k, v)| (k, v))
    }

    /// Check invariants for the withdrawal queue.
    ///
    /// Validates:
    /// - `next_withdraw_to_execute <= next_pending_withdrawal_id`
    /// - If non-empty, head ID exists in the map
    ///
    /// # Returns
    /// `true` if all invariants hold.
    #[must_use]
    pub fn check_invariants(&self) -> bool {
        // next_withdraw_to_execute <= next_pending_withdrawal_id
        if self.next_withdraw_to_execute > self.next_pending_withdrawal_id {
            return false;
        }

        // If non-empty, the head must exist
        if !self.is_empty()
            && !self
                .pending_withdrawals
                .contains_key(&self.next_withdraw_to_execute)
        {
            return false;
        }

        true
    }

    /// Check invariants including the max pending limit.
    ///
    /// # Arguments
    /// * `max_pending` - Maximum allowed pending withdrawals.
    ///
    /// # Returns
    /// `true` if all invariants hold including queue length bounds.
    #[must_use]
    pub fn check_invariants_with_max(&self, max_pending: u32) -> bool {
        // Check basic invariants first
        if !self.check_invariants() {
            return false;
        }

        // pending_withdrawals.len() <= max_pending_withdrawals <= MAX_PENDING
        let len = self.pending_withdrawals.len();
        if len > (max_pending as usize) || (max_pending as usize) > MAX_PENDING {
            return false;
        }

        true
    }

    /// Compute aggregate queue statistics.
    ///
    /// # Returns
    /// `QueueStatus` with totals.
    #[must_use]
    pub fn status(&self) -> QueueStatus {
        compute_queue_status(self.pending_withdrawals.values())
    }

    /// Get total escrowed shares across all pending withdrawals.
    ///
    /// # Returns
    /// Total escrow shares.
    #[must_use]
    pub fn total_escrow_shares(&self) -> u128 {
        self.pending_withdrawals
            .values()
            .map(|w| w.escrow_shares)
            .fold(0u128, |acc, x| acc.saturating_add(x))
    }

    /// Get total expected assets across all pending withdrawals.
    ///
    /// # Returns
    /// Total expected assets.
    #[must_use]
    pub fn total_expected_assets(&self) -> u128 {
        self.pending_withdrawals
            .values()
            .map(|w| w.expected_assets)
            .fold(0u128, |acc, x| acc.saturating_add(x))
    }
}

/// Errors that can occur during queue operations.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueError {
    /// Queue is at maximum capacity.
    QueueFull { current: u32, max: u32 },
    /// Withdrawal ID not found.
    WithdrawalNotFound { id: u64 },
    /// Queue is empty.
    QueueEmpty,
    /// Invariant violation detected.
    InvariantViolation { message: alloc::string::String },
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
fn owner_addr(index: u64) -> Address {
    let mut addr = [0u8; 32];
    addr[0] = 0x11;
    addr[1..9].copy_from_slice(&index.to_le_bytes());
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn make_withdrawal(owner: u8, shares: u128, expected: u128) -> PendingWithdrawal {
        PendingWithdrawal::new(
            owner_addr(owner as u64),
            owner_addr(owner as u64),
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
        assert!(!is_past_cooldown(
            requested,
            requested + cooldown - 1,
            cooldown
        ));

        // Past cooldown
        assert!(is_past_cooldown(requested, requested + cooldown, cooldown));
        assert!(is_past_cooldown(
            requested,
            requested + cooldown + 1,
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

        // Zero expected (edge case, treated as full burn)
        let settlement = compute_settlement(100, 0, 500);
        assert_eq!(settlement.to_burn, 100);
        assert_eq!(settlement.refund, 0);
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

    // =========================================================================
    // WithdrawQueue Tests
    // =========================================================================

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
                1_000_000_000_000,
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
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        let id2 = queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();
        let id3 = queue
            .enqueue(
                owner_addr(3),
                owner_addr(3),
                300,
                3000,
                3_000_000_000_000,
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
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

        // Should fail when full
        let result = queue.enqueue(
            owner_addr(3),
            owner_addr(3),
            300,
            3000,
            3_000_000_000_000,
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
    fn test_withdraw_queue_peek() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        // Empty queue
        assert!(queue.peek().is_none());

        // Add items
        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

        // Peek should return the first item
        let (id, withdrawal) = queue.peek().unwrap();
        assert_eq!(id, 0);
        assert_eq!(withdrawal.owner, owner_addr(1));
        assert_eq!(withdrawal.escrow_shares, 100);

        // Peek again should return the same item
        let (id2, _) = queue.peek().unwrap();
        assert_eq!(id2, 0);
        assert_eq!(queue.len(), 2); // Length unchanged
    }

    #[test]
    fn test_withdraw_queue_head() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();

        let (id, withdrawal) = queue.head().unwrap();
        assert_eq!(id, 0);
        assert_eq!(withdrawal.owner, owner_addr(1));
    }

    #[test]
    fn test_withdraw_queue_dequeue() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        // Empty queue
        assert!(queue.dequeue().is_none());

        // Add items
        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(3),
                owner_addr(3),
                300,
                3000,
                3_000_000_000_000,
                max_pending,
            )
            .unwrap();

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
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

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
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();

        assert!(queue.contains(0));
        assert!(!queue.contains(1));
        assert!(!queue.contains(999));
    }

    #[test]
    fn test_withdraw_queue_iter() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

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
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(3),
                owner_addr(3),
                300,
                3000,
                3_000_000_000_000,
                max_pending,
            )
            .unwrap();

        let status = queue.status();
        assert_eq!(status.length, 3);
        assert_eq!(status.total_expected_assets, 6000);
        assert_eq!(status.total_escrow_shares, 600);
    }

    #[test]
    fn test_withdraw_queue_total_escrow_shares() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

        assert_eq!(queue.total_escrow_shares(), 300);
    }

    #[test]
    fn test_withdraw_queue_total_expected_assets() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

        assert_eq!(queue.total_expected_assets(), 3000);
    }

    #[test]
    fn test_withdraw_queue_check_invariants() {
        let mut queue = WithdrawQueue::new();
        assert!(queue.check_invariants());

        // After enqueue
        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                100,
            )
            .unwrap();
        assert!(queue.check_invariants());

        // After dequeue
        queue.dequeue();
        assert!(queue.check_invariants());
    }

    #[test]
    fn test_withdraw_queue_check_invariants_with_max() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();
        queue
            .enqueue(
                owner_addr(2),
                owner_addr(2),
                200,
                2000,
                2_000_000_000_000,
                max_pending,
            )
            .unwrap();

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
                1_000_000_000_000,
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
        let max_pending = 100u32;

        // Enqueue in order
        for i in 0..5 {
            queue
                .enqueue(
                    owner_addr(i as u64),
                    owner_addr(i as u64),
                    (i as u128 + 1) * 100,
                    (i as u128 + 1) * 1000,
                    (i as u64 + 1) * 1_000_000_000_000,
                    max_pending,
                )
                .unwrap();
        }

        // Dequeue should maintain FIFO order
        for i in 0..5 {
            let (id, w) = queue.dequeue().unwrap();
            assert_eq!(id, i);
            assert_eq!(w.owner, owner_addr(i as u64));
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
            1_000_000_000_000,
        );

        let id = queue.enqueue_withdrawal(w.clone(), max_pending).unwrap();
        assert_eq!(id, 0);

        let stored = queue.get(0).unwrap();
        assert_eq!(stored.owner, owner_addr(1));
        assert_eq!(stored.receiver, owner_addr(2));
    }

    #[test]
    fn test_withdraw_queue_get_mut() {
        let mut queue = WithdrawQueue::new();
        let max_pending = 100u32;

        queue
            .enqueue(
                owner_addr(1),
                owner_addr(1),
                100,
                1000,
                1_000_000_000_000,
                max_pending,
            )
            .unwrap();

        // Modify the withdrawal
        if let Some(w) = queue.get_mut(0) {
            w.escrow_shares = 200;
        }

        // Verify modification
        let w = queue.get(0).unwrap();
        assert_eq!(w.escrow_shares, 200);
    }

    #[test]
    fn test_withdraw_queue_empty_operations() {
        let queue = WithdrawQueue::new();

        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(queue.peek().is_none());
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
}

// ============================================================================
// Property Tests
// ============================================================================

#[cfg(test)]
mod proptests {
    use super::*;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    fn addr_with_tag(tag: u8, index: u64) -> Address {
        let mut addr = [0u8; 32];
        addr[0] = tag;
        addr[1..9].copy_from_slice(&index.to_le_bytes());
        addr
    }

    fn owner_addr(index: u64) -> Address {
        addr_with_tag(0x11, index)
    }

    fn receiver_addr(index: u64) -> Address {
        addr_with_tag(0x22, index)
    }

    /// Strategy for generating a PendingWithdrawal
    fn arb_withdrawal() -> impl Strategy<Value = PendingWithdrawal> {
        (
            1u32..1000u32,                            // owner index
            1u128..=u64::MAX as u128,                 // shares
            MIN_WITHDRAWAL_ASSETS..=u64::MAX as u128, // expected_assets
            0u64..u64::MAX,                           // timestamp
        )
            .prop_map(|(owner_idx, shares, expected, ts)| {
                PendingWithdrawal::new(
                    owner_addr(owner_idx as u64),
                    owner_addr(owner_idx as u64),
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
                owner_addr(1),
                receiver_addr(1),
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
                owner_addr(1),
                receiver_addr(1),
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
                owner_addr(1),
                receiver_addr(1),
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
                owner_addr(1),
                receiver_addr(1),
                shares,
                expected,
                0,
            );
            let result = compute_partial_withdrawal(&w, available);

            prop_assert!(result.assets_out <= expected);
            prop_assert!(result.assets_out <= available);
        }

        // ===================================================================
        // WithdrawQueue Property Tests
        // ===================================================================

        // ===================================================================
        // Property: enqueue increases length by 1
        // Invariant: len(after) = len(before) + 1
        // ===================================================================
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
                    i as u64,
                    max_pending,
                ).unwrap();
                prop_assert_eq!(queue.len(), len_before + 1);
            }
        }

        // ===================================================================
        // Property: dequeue decreases length by 1
        // Invariant: len(after) = len(before) - 1 when non-empty
        // ===================================================================
        #[test]
        fn withdraw_queue_dequeue_decreases_length(
            num_enqueues in 1usize..20usize,
        ) {
            let mut queue = WithdrawQueue::new();
            let max_pending = 100u32;

            // Enqueue items
            for i in 0..num_enqueues {
                queue.enqueue(
                    owner_addr(i as u64),
                    receiver_addr(i as u64),
                    100,
                    1000,
                    i as u64,
                    max_pending,
                ).unwrap();
            }

            // Dequeue all
            for _ in 0..num_enqueues {
                let len_before = queue.len();
                queue.dequeue();
                prop_assert_eq!(queue.len(), len_before - 1);
            }
        }

        // ===================================================================
        // Property: invariants hold after any sequence of enqueue/dequeue
        // Invariant: check_invariants() returns true
        // ===================================================================
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
                        owner_addr(counter as u64),
                        receiver_addr(counter as u64),
                        100,
                        1000,
                        counter,
                        max_pending,
                    ).unwrap();
                    counter += 1;
                } else if op == 1 && !queue.is_empty() {
                    queue.dequeue();
                }
                prop_assert!(queue.check_invariants(), "Invariant violated after operation");
            }
        }

        // ===================================================================
        // Property: FIFO ordering is preserved
        // Invariant: dequeue returns items in insertion order
        // ===================================================================
        #[test]
        fn withdraw_queue_fifo_ordering(
            num_items in 1usize..20usize,
        ) {
            let mut queue = WithdrawQueue::new();
            let max_pending = 100u32;

            // Enqueue with sequential IDs
            for i in 0..num_items {
                queue.enqueue(
                    owner_addr(i as u64),
                    receiver_addr(i as u64),
                    (i as u128) + 1,
                    (i as u128 + 1) * 1000,
                    i as u64,
                    max_pending,
                ).unwrap();
            }

            // Dequeue should return in FIFO order
            for i in 0..num_items {
                let (id, w) = queue.dequeue().unwrap();
                prop_assert_eq!(id, i as u64, "ID mismatch at position {}", i);
                prop_assert_eq!(w.owner, owner_addr(i as u64), "Owner mismatch at position {}", i);
            }
        }

        // ===================================================================
        // Property: next_pending_withdrawal_id is monotonic
        // Invariant: ID always increases
        // ===================================================================
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
                    i as u64,
                    max_pending,
                ).unwrap();

                if let Some(prev) = last_id {
                    prop_assert!(id > prev, "ID not monotonically increasing");
                }
                last_id = Some(id);
            }
        }

        // ===================================================================
        // Property: next_withdraw_to_execute <= next_pending_withdrawal_id
        // Invariant: head pointer never exceeds next ID
        // ===================================================================
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
                        owner_addr(counter as u64),
                        receiver_addr(counter as u64),
                        100,
                        1000,
                        counter,
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

        // ===================================================================
        // Property: total_escrow_shares equals sum of all escrow_shares
        // Invariant: Aggregation is correct
        // ===================================================================
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

        // ===================================================================
        // Property: total_expected_assets equals sum of all expected_assets
        // Invariant: Aggregation is correct
        // ===================================================================
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

        // ===================================================================
        // Property: queue length bounded by max_pending
        // Invariant: len <= max_pending
        // ===================================================================
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
                    i as u64,
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

        // ===================================================================
        // Property: peek returns the same value as head
        // Invariant: peek() == head()
        // ===================================================================
        #[test]
        fn withdraw_queue_peek_equals_head(
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
                    i as u64,
                    max_pending,
                ).unwrap();
            }

            let peek_result = queue.peek();
            let head_result = queue.head();

            prop_assert_eq!(peek_result, head_result);
        }

        // ===================================================================
        // Property: get returns correct withdrawal by ID
        // Invariant: get(id) returns the withdrawal with that ID
        // ===================================================================
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
                    i as u64,
                    max_pending,
                ).unwrap();
                ids.push(id);
            }

            // Verify each ID returns the correct withdrawal
            for (i, id) in ids.iter().enumerate() {
                let w = queue.get(*id).unwrap();
                prop_assert_eq!(&w.owner, &owner_addr(i as u64));
                prop_assert_eq!(w.escrow_shares, (i as u128) + 1);
            }
        }

        // ===================================================================
        // Property: status matches queue contents
        // Invariant: status.length == len() and totals match
        // ===================================================================
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
}
