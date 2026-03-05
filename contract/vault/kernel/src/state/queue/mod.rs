//! Chain-agnostic withdrawal queue types and pure logic functions.
//!
//! This module provides data structures for pending withdrawals and pure
//! functions for queue logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;
#[cfg(feature = "borsh-schema")]
use borsh::BorshSchema;
#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::math::number::Number;
use crate::math::wad::Wad;
use crate::types::{Address, EscrowSettlement, TimestampNs};

/// Minimum withdrawal amount in base asset units to prevent dust.
/// Withdrawals below this threshold should be rejected.
pub const MIN_WITHDRAWAL_ASSETS: u128 = 1_000;

/// Maximum queue length before rejecting new requests.
///
/// This is a legacy alias of [`MAX_PENDING`] to keep queue helpers consistent
/// with the kernel config limit and avoid ambiguous capacity thresholds.
pub const MAX_QUEUE_LENGTH: u32 = crate::state::vault::MAX_PENDING as u32;

/// Default cooldown period in nanoseconds (24 hours).
/// Withdrawals cannot be processed until this time has elapsed.
pub const DEFAULT_COOLDOWN_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

/// A pending withdrawal request in the queue.
///
/// Represents a user's request to redeem shares for underlying assets.
/// The shares are held in escrow until the withdrawal is processed.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "borsh-schema", derive(BorshSchema))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingWithdrawal {
    pub owner: Address,
    pub receiver: Address,
    pub escrow_shares: u128,
    pub expected_assets: u128,
    pub requested_at_ns: TimestampNs,
}

impl PendingWithdrawal {
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
}

/// Result of attempting to satisfy a withdrawal from available assets.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawalResult {
    pub assets_out: u128,
    pub settlement: EscrowSettlement,
}

/// Status information for a single withdrawal request in the queue.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawalRequestStatus {
    pub index: u32,
    pub depth_assets: u128,
    pub withdrawal: PendingWithdrawal,
}

/// Aggregate status of the entire withdrawal queue.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct QueueStatus {
    pub length: u32,
    pub total_expected_assets: u128,
    pub total_escrow_shares: u128,
}

#[inline]
#[must_use]
pub fn is_valid_withdrawal_amount(assets: u128) -> bool {
    assets >= MIN_WITHDRAWAL_ASSETS
}

#[inline]
#[must_use]
pub fn can_enqueue(current_length: u32) -> bool {
    current_length < MAX_QUEUE_LENGTH
}

#[inline]
#[must_use]
pub fn is_past_cooldown(
    requested_at_ns: TimestampNs,
    now_ns: TimestampNs,
    cooldown_ns: u64,
) -> bool {
    now_ns >= requested_at_ns.saturating_add(cooldown_ns)
}

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

// Pure Functions - Settlement Computation

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
#[inline]
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

    // Partial redemption - burn proportional shares, refund the rest.
    // Use ceil to avoid zero-burn partials (assets out without burning shares).
    // shares_to_burn = ceil(escrow_shares * actual_assets / expected_assets)
    let shares_to_burn = Number::mul_div_ceil(
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
/// * `share_price_wad` - Current share price as a WAD (1e18 = 1.0).
/// * `original_share_price_wad` - Share price at time of request.
///
/// # Returns
/// `EscrowSettlement` based on price ratio.
#[inline]
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

    // Partial burn: ratio of current to original price.
    // Use ceil to avoid zero-burn partials (consistent with compute_settlement).
    // shares_to_burn = ceil(escrow_shares * current_price / original_price)
    let shares_to_burn = Number::mul_div_ceil(
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

// Pure Functions - Queue Aggregation

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

// Queue Storage Types

use alloc::vec::Vec;

pub use crate::state::vault::MAX_PENDING;

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "borsh-schema", derive(BorshSchema))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PendingWithdrawals {
    entries: Vec<PendingWithdrawalEntry>,
}

#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "borsh-schema", derive(BorshSchema))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
struct PendingWithdrawalEntry {
    id: u64,
    withdrawal: PendingWithdrawal,
}

impl PendingWithdrawals {
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    fn locate(&self, id: u64) -> Result<usize, usize> {
        self.entries.binary_search_by(|entry| entry.id.cmp(&id))
    }

    pub fn insert(&mut self, id: u64, withdrawal: PendingWithdrawal) -> Option<PendingWithdrawal> {
        match self.locate(id) {
            Ok(index) => {
                let old = core::mem::replace(&mut self.entries[index].withdrawal, withdrawal);
                Some(old)
            }
            Err(index) => {
                self.entries
                    .insert(index, PendingWithdrawalEntry { id, withdrawal });
                None
            }
        }
    }

    pub fn remove(&mut self, id: &u64) -> Option<PendingWithdrawal> {
        self.locate(*id)
            .ok()
            .map(|index| self.entries.remove(index).withdrawal)
    }

    #[inline]
    #[must_use]
    pub fn get(&self, id: &u64) -> Option<&PendingWithdrawal> {
        self.locate(*id)
            .ok()
            .map(|index| &self.entries[index].withdrawal)
    }

    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, id: &u64) -> Option<&mut PendingWithdrawal> {
        self.locate(*id)
            .ok()
            .map(|index| &mut self.entries[index].withdrawal)
    }

    #[inline]
    #[must_use]
    pub fn contains_key(&self, id: &u64) -> bool {
        self.locate(*id).is_ok()
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &PendingWithdrawal)> {
        self.entries
            .iter()
            .map(|entry| (&entry.id, &entry.withdrawal))
    }

    #[inline]
    pub fn values(&self) -> impl Iterator<Item = &PendingWithdrawal> {
        self.entries.iter().map(|entry| &entry.withdrawal)
    }

    #[inline]
    pub fn keys(&self) -> impl Iterator<Item = &u64> {
        self.entries.iter().map(|entry| &entry.id)
    }
}

impl FromIterator<(u64, PendingWithdrawal)> for PendingWithdrawals {
    fn from_iter<T: IntoIterator<Item = (u64, PendingWithdrawal)>>(iter: T) -> Self {
        let mut entries: Vec<PendingWithdrawalEntry> = iter
            .into_iter()
            .map(|(id, withdrawal)| PendingWithdrawalEntry { id, withdrawal })
            .collect();
        entries.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        entries.dedup_by(|a, b| a.id == b.id);
        Self { entries }
    }
}

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
/// - `cached_total_escrow == sum(pending_withdrawals.values().map(|w| w.escrow_shares))`
/// - `cached_total_expected == sum(pending_withdrawals.values().map(|w| w.expected_assets))`
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "borsh-schema", derive(BorshSchema))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawQueue {
    /// Pending withdrawals keyed by monotonic ID.
    pub pending_withdrawals: PendingWithdrawals,
    /// ID of the next withdrawal to execute (queue head).
    pub next_withdraw_to_execute: u64,
    /// Next ID to allocate for new withdrawals (monotonic, never decremented).
    pub next_pending_withdrawal_id: u64,
    /// Cached total of escrow shares across all pending withdrawals.
    /// Maintained incrementally on enqueue/dequeue for O(1) lookups.
    cached_total_escrow: u128,
    /// Cached total of expected assets across all pending withdrawals.
    /// Maintained incrementally on enqueue/dequeue for O(1) lookups.
    cached_total_expected: u128,
}

impl Default for WithdrawQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Sum escrow shares and expected assets across an iterator of pending withdrawals.
fn compute_pending_totals<'a>(iter: impl Iterator<Item = &'a PendingWithdrawal>) -> (u128, u128) {
    iter.fold((0u128, 0u128), |(esc, exp), w| {
        (
            esc.saturating_add(w.escrow_shares),
            exp.saturating_add(w.expected_assets),
        )
    })
}

impl WithdrawQueue {
    /// Create a new empty withdrawal queue.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending_withdrawals: PendingWithdrawals::new(),
            next_withdraw_to_execute: 0,
            next_pending_withdrawal_id: 0,
            cached_total_escrow: 0,
            cached_total_expected: 0,
        }
    }

    /// Create a queue with initial state (for testing or recovery).
    #[must_use]
    pub fn with_state<I>(
        pending_withdrawals: I,
        next_withdraw_to_execute: u64,
        next_pending_withdrawal_id: u64,
    ) -> Self
    where
        I: IntoIterator<Item = (u64, PendingWithdrawal)>,
    {
        let pending_withdrawals: PendingWithdrawals = pending_withdrawals.into_iter().collect();
        let (cached_total_escrow, cached_total_expected) =
            compute_pending_totals(pending_withdrawals.values());
        Self {
            pending_withdrawals,
            next_withdraw_to_execute,
            next_pending_withdrawal_id,
            cached_total_escrow,
            cached_total_expected,
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
    /// Convenience wrapper that constructs a `PendingWithdrawal` and delegates
    /// to [`enqueue_withdrawal`](Self::enqueue_withdrawal).
    pub fn enqueue(
        &mut self,
        owner: Address,
        receiver: Address,
        escrow_shares: u128,
        expected_assets: u128,
        requested_at_ns: TimestampNs,
        max_pending: u32,
    ) -> Result<u64, QueueError> {
        let withdrawal = PendingWithdrawal::new(
            owner,
            receiver,
            escrow_shares,
            expected_assets,
            requested_at_ns,
        );
        self.enqueue_withdrawal(withdrawal, max_pending)
    }

    /// Enqueue a pre-constructed pending withdrawal.
    ///
    /// Allocates a new monotonic ID and inserts the withdrawal at the tail.
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

        // Compute cache totals first so we can fail without mutating queue state.
        let new_escrow = self
            .cached_total_escrow
            .checked_add(withdrawal.escrow_shares)
            .ok_or(QueueError::CacheOverflow)?;
        let new_expected = self
            .cached_total_expected
            .checked_add(withdrawal.expected_assets)
            .ok_or(QueueError::CacheOverflow)?;

        self.pending_withdrawals.insert(id, withdrawal);
        self.next_pending_withdrawal_id = self.next_pending_withdrawal_id.saturating_add(1);

        // Update cached totals (overflow already checked)
        self.cached_total_escrow = new_escrow;
        self.cached_total_expected = new_expected;

        Ok(id)
    }

    /// Get the head of the queue without removing it.
    ///
    /// # Returns
    /// `Some((id, &withdrawal))` if non-empty, `None` if empty.
    #[inline]
    #[must_use]
    pub fn head(&self) -> Option<(u64, &PendingWithdrawal)> {
        self.pending_withdrawals
            .get(&self.next_withdraw_to_execute)
            .map(|w| (self.next_withdraw_to_execute, w))
    }

    /// Dequeue and return the head of the queue (FIFO).
    ///
    /// Removes the head and advances `next_withdraw_to_execute` to the next
    /// available ID in the queue (or to `next_pending_withdrawal_id` if empty).
    ///
    /// # Returns
    /// `Some((id, withdrawal))` if non-empty, `None` if empty.
    ///
    pub fn dequeue(&mut self) -> Option<(u64, PendingWithdrawal)> {
        if self.is_empty() {
            return None;
        }

        let head_id = self.next_withdraw_to_execute;
        let withdrawal = self.pending_withdrawals.remove(&head_id)?;

        #[cfg(test)]
        {
            self.cached_total_escrow = self
                .cached_total_escrow
                .checked_sub(withdrawal.escrow_shares)
                .expect("dequeue: cached_total_escrow underflow — queue cache corrupt");
            self.cached_total_expected = self
                .cached_total_expected
                .checked_sub(withdrawal.expected_assets)
                .expect("dequeue: cached_total_expected underflow — queue cache corrupt");
        }
        #[cfg(not(test))]
        {
            self.cached_total_escrow = self
                .cached_total_escrow
                .saturating_sub(withdrawal.escrow_shares);
            self.cached_total_expected = self
                .cached_total_expected
                .saturating_sub(withdrawal.expected_assets);
        }

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
    pub fn iter(&self) -> impl Iterator<Item = (u64, &PendingWithdrawal)> {
        self.pending_withdrawals.iter().map(|(k, v)| (*k, v))
    }

    /// Check invariants for the withdrawal queue.
    ///
    /// Validates:
    /// - `next_withdraw_to_execute <= next_pending_withdrawal_id`
    /// - If non-empty, head ID exists in the map
    /// - Cached totals match computed totals
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

        // Verify cached totals match computed totals
        let (computed_escrow, computed_expected) =
            compute_pending_totals(self.pending_withdrawals.values());

        if self.cached_total_escrow != computed_escrow {
            return false;
        }
        if self.cached_total_expected != computed_expected {
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
    #[inline]
    #[must_use]
    pub fn status(&self) -> QueueStatus {
        QueueStatus {
            length: self.pending_withdrawals.len() as u32,
            total_expected_assets: self.cached_total_expected,
            total_escrow_shares: self.cached_total_escrow,
        }
    }

    /// Get total escrowed shares across all pending withdrawals.
    ///
    /// Returns cached value in O(1) time.
    ///
    /// # Returns
    /// Total escrow shares.
    #[inline]
    #[must_use]
    pub fn total_escrow_shares(&self) -> u128 {
        self.cached_total_escrow
    }

    /// Get total expected assets across all pending withdrawals.
    ///
    /// Returns cached value in O(1) time.
    ///
    /// # Returns
    /// Total expected assets.
    #[inline]
    #[must_use]
    pub fn total_expected_assets(&self) -> u128 {
        self.cached_total_expected
    }
}

/// Errors that can occur during queue operations.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum QueueError {
    /// Queue is at maximum capacity.
    QueueFull { current: u32, max: u32 },
    /// Withdrawal ID not found.
    WithdrawalNotFound { id: u64 },
    /// Queue is empty.
    QueueEmpty,
    /// Invariant violation detected.
    InvariantViolation { message: alloc::string::String },
    /// Cached total overflow.
    CacheOverflow,
}

#[cfg(test)]
mod tests;
