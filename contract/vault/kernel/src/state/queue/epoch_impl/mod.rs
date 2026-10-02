//! Chain-agnostic withdrawal queue types and pure logic functions.
//!
//! This module provides data structures for pending withdrawals and pure
//! functions for queue logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

#[cfg(feature = "borsh-schema")]
use alloc::string::ToString;

use crate::state::settlement::{EpochId, EpochState, RequestError};
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

/// Default cooldown period in nanoseconds (1 hour).
/// Withdrawals cannot be processed until this time has elapsed.
pub const DEFAULT_COOLDOWN_NS: u64 = 60 * 60 * 1_000_000_000;

/// A pending, unpriced withdrawal request in the queue.
///
/// Escrowed shares are held until settlement. The entry records identity,
/// escrow amount, a caller-declared minimum asset bound (`min_assets_out`,
/// a slippage floor for settlement, never a claim), the request time, and
/// the epoch the request was queued in. There is no asset-denominated
/// claim field, so a pre-settlement fixed claim is unrepresentable.
///
/// Legacy requests migrated from pre-epoch storage are constructed only
/// through [`PendingWithdrawal::migrated_legacy`]: the legacy asset quote
/// is dropped, `min_assets_out` is pinned to zero, and the entry is tagged
/// with [`EpochId::MIGRATION_INTAKE`]. Migrated entries settle against the
/// first settled epoch and are repaid first because they hold the FIFO
/// head positions they already held.
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct PendingWithdrawal {
    pub owner: Address,
    pub receiver: Address,
    pub escrow_shares: u128,
    pub min_assets_out: u128,
    pub requested_at_ns: TimestampNs,
    pub epoch_id: EpochId,
}

impl PendingWithdrawal {
    /// Construct an unpriced withdrawal request bound to a settlement
    /// epoch. Requires at least one escrowed share.
    #[inline]
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn new(
        owner: Address,
        receiver: Address,
        escrow_shares: u128,
        min_assets_out: u128,
        requested_at_ns: TimestampNs,
        epoch_id: EpochId,
    ) -> Result<Self, RequestError> {
        if escrow_shares == 0 {
            return Err(RequestError::ZeroEscrowShares);
        }
        if !epoch_id.is_settlement_epoch() {
            return Err(RequestError::InvalidEpoch);
        }
        Ok(Self {
            owner,
            receiver,
            escrow_shares,
            min_assets_out,
            requested_at_ns,
            epoch_id,
        })
    }

    /// Migration-only constructor for legacy fixed-claim requests.
    ///
    /// Preserves identity, escrowed shares, and the original request time.
    /// The legacy asset quote is deliberately absent from the signature,
    /// cannot be stored on the entry, and therefore cannot enter new
    /// settlement math. `min_assets_out` is pinned to zero and the entry
    /// is tagged [`EpochId::MIGRATION_INTAKE`], which marks it distinct
    /// from new intake and settles it against the first settled epoch.
    #[inline]
    #[must_use = "return value binds or validates settlement law; ignoring it can admit an unpriced claim"]
    pub fn migrated_legacy(
        owner: Address,
        receiver: Address,
        escrow_shares: u128,
        requested_at_ns: TimestampNs,
    ) -> Result<Self, RequestError> {
        if escrow_shares == 0 {
            return Err(RequestError::ZeroEscrowShares);
        }
        Ok(Self {
            owner,
            receiver,
            escrow_shares,
            min_assets_out: 0,
            requested_at_ns,
            epoch_id: EpochId::MIGRATION_INTAKE,
        })
    }

    /// True when this entry originates from legacy migration intake.
    #[inline]
    #[must_use]
    pub fn is_migrated_legacy(&self) -> bool {
        self.epoch_id == EpochId::MIGRATION_INTAKE
    }
}

/// Result of attempting to satisfy a withdrawal from available assets.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawalResult {
    pub assets_out: u128,
    pub settlement: EscrowSettlement,
}

/// Compute escrow settlement for paying a settled claim, given the assets
/// available at execution time.
///
/// `settled_claim_assets` must be recomputed at execution time from the
/// accepted epoch settlement (see [`settled_claim`]); it is never a stored
/// request-time quote.
#[inline]
#[must_use]
pub fn compute_idle_settlement(
    escrow_shares: u128,
    settled_claim_assets: u128,
    available_assets: u128,
) -> Option<WithdrawalResult> {
    if settled_claim_assets == 0 {
        return Some(WithdrawalResult {
            assets_out: 0,
            settlement: EscrowSettlement::refund_all(escrow_shares),
        });
    }

    if available_assets >= settled_claim_assets {
        return Some(WithdrawalResult {
            assets_out: settled_claim_assets,
            settlement: EscrowSettlement::burn_all(escrow_shares),
        });
    }

    if available_assets == 0 {
        return None;
    }

    let assets_out = available_assets;
    Some(WithdrawalResult {
        assets_out,
        settlement: compute_settlement(escrow_shares, settled_claim_assets, assets_out),
    })
}

/// Status information for a single withdrawal request in the queue.
///
/// Queue depth is denominated in escrowed shares. No asset figure exists
/// for an unpriced request before settlement, so no asset-based queue
/// position (a de facto payout estimate) can leak or be consumed by
/// non-repricing paths.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawalRequestStatus {
    pub index: u32,
    pub depth_escrow_shares: u128,
    pub withdrawal: PendingWithdrawal,
}

/// Aggregate status of the entire withdrawal queue.
///
/// Deliberately share-denominated: a persisted aggregate asset figure is
/// the fixed-claim class ENG-697 removes.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct QueueStatus {
    pub length: u32,
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
    now_ns >= requested_at_ns.saturating_add_u64(cooldown_ns)
}

/// Recompute the settled claim for an unpriced request from the epoch state.
///
/// Returns `None` until an accepted settlement covers the request's epoch
/// (for migration-intake entries: until the first epoch settles). This is
/// the only path from escrowed shares to assets; no stored claim exists.
#[must_use]
pub fn settled_claim(withdrawal: &PendingWithdrawal, epoch_state: &EpochState) -> Option<u128> {
    epoch_state.settled_claim_for(withdrawal.epoch_id, withdrawal.escrow_shares)
}

/// Check whether a settled claim can be paid in full from available assets.
///
/// Claims exist only after accepted epoch settlement and are floored at
/// the request's `min_assets_out` slippage bound.
#[must_use]
pub fn can_satisfy_withdrawal(
    withdrawal: &PendingWithdrawal,
    epoch_state: &EpochState,
    available_assets: u128,
) -> bool {
    let Some(claim) = settled_claim(withdrawal, epoch_state) else {
        return false;
    };
    claim >= withdrawal.min_assets_out && available_assets >= claim
}

/// Check whether partial settlement is meaningful: available assets are
/// non-zero and strictly below the settled claim of a settled request.
#[must_use]
pub fn can_partially_satisfy(
    withdrawal: &PendingWithdrawal,
    epoch_state: &EpochState,
    available_assets: u128,
) -> bool {
    match settled_claim(withdrawal, epoch_state) {
        Some(claim) => available_assets > 0 && available_assets < claim,
        None => false,
    }
}

/// Count how many queued requests can be fully paid, in FIFO order, from
/// the available asset budget using execution-time recomputed claims.
///
/// A request whose epoch has not settled, or whose settled claim falls
/// below its `min_assets_out` floor, stops the run: the FIFO head cannot
/// be skipped and later requests cannot jump ahead of an unsettled head.
#[must_use]
pub fn count_satisfiable<'a, I>(
    withdrawals: I,
    epoch_state: &EpochState,
    available_assets: u128,
) -> (u32, u128)
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut count = 0u32;
    let mut total_assets = 0u128;

    for withdrawal in withdrawals {
        let Some(claim) = settled_claim(withdrawal, epoch_state) else {
            break;
        };
        if claim < withdrawal.min_assets_out {
            break;
        }
        let Some(new_total) = total_assets.checked_add(claim) else {
            break;
        };
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
/// versus the settled claim amount.
///
/// # Arguments
/// * `escrow_shares` - Total shares held in escrow.
/// * `settled_assets` - Assets owed under the recomputed settled claim.
///   Must be derived at execution time from the accepted epoch settlement
///   (see [`settled_claim`]), never from a stored request-time quote.
/// * `actual_assets` - Assets actually being redeemed.
///
/// # Returns
/// `EscrowSettlement` with shares to burn and shares to refund.
///
/// # Logic
/// - If actual >= settled claim: burn all shares (full redemption).
/// - If actual < settled claim: burn proportional shares, refund the rest.
/// - If actual == 0: refund all shares (cancellation).
#[inline]
#[must_use]
pub fn compute_settlement(
    escrow_shares: u128,
    settled_assets: u128,
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

    if settled_assets == 0 {
        return EscrowSettlement::refund_all(escrow_shares);
    }

    if actual_assets >= settled_assets {
        // Full redemption - burn all shares
        return EscrowSettlement::burn_all(escrow_shares);
    }

    // Partial redemption - burn proportional shares, refund the rest.
    // Use ceil to avoid zero-burn partials (assets out without burning shares).
    // shares_to_burn = ceil(escrow_shares * actual_assets / settled_assets)
    let shares_to_burn = Number::mul_div_ceil(
        Number::from(escrow_shares),
        Number::from(actual_assets),
        Number::from(settled_assets),
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

/// Compute the withdrawal result for a fully satisfied settled claim.
///
/// The claim is recomputed at execution time from `epoch_state`; nothing
/// is paid while the request's epoch (or, for migration-intake entries,
/// the first settlement) is unsettled, and claims below the request's
/// `min_assets_out` floor are rejected.
#[must_use]
pub fn compute_full_withdrawal(
    withdrawal: &PendingWithdrawal,
    epoch_state: &EpochState,
    available_assets: u128,
) -> Option<WithdrawalResult> {
    let claim = settled_claim(withdrawal, epoch_state)?;
    if claim < withdrawal.min_assets_out {
        return None;
    }
    compute_idle_settlement(withdrawal.escrow_shares, claim, available_assets)
        .filter(|result| result.assets_out == claim)
}

/// Compute the withdrawal result for a partial redemption against the
/// recomputed settled claim.
///
/// Returns `assets_out: 0` with a full refund when the request's epoch is
/// unsettled (no claim exists yet to settle against).
#[must_use]
pub fn compute_partial_withdrawal(
    withdrawal: &PendingWithdrawal,
    epoch_state: &EpochState,
    available_assets: u128,
) -> WithdrawalResult {
    let claim = settled_claim(withdrawal, epoch_state).unwrap_or(0);
    let actual_assets = available_assets.min(claim);
    let settlement = compute_settlement(withdrawal.escrow_shares, claim, actual_assets);
    WithdrawalResult {
        assets_out: actual_assets,
        settlement,
    }
}

// Pure Functions - Queue Aggregation

/// Compute aggregate queue status from an iterator of withdrawals.
///
/// Share-denominated only: pending escrow, never an asset forecast.
#[must_use]
pub fn compute_queue_status<'a, I>(withdrawals: I) -> QueueStatus
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut status = QueueStatus::default();

    for withdrawal in withdrawals {
        status.length = status.length.saturating_add(1);
        status.total_escrow_shares = status
            .total_escrow_shares
            .saturating_add(withdrawal.escrow_shares);
    }

    status
}

/// Find a withdrawal request's status by owner.
///
/// Depth is denominated in escrowed shares, not assets: no asset figure
/// exists for an unpriced request before settlement.
#[must_use]
pub fn find_request_status<'a, I>(
    withdrawals: I,
    owner: &Address,
) -> Option<WithdrawalRequestStatus>
where
    I: IntoIterator<Item = &'a PendingWithdrawal>,
{
    let mut index = 0u32;
    let mut depth_escrow_shares = 0u128;

    for withdrawal in withdrawals {
        if &withdrawal.owner == owner {
            return Some(WithdrawalRequestStatus {
                index,
                depth_escrow_shares,
                withdrawal: withdrawal.clone(),
            });
        }
        depth_escrow_shares = depth_escrow_shares.saturating_add(withdrawal.escrow_shares);
        index = index.saturating_add(1);
    }

    None
}

// Queue Storage Types

use alloc::vec::Vec;

pub use crate::state::vault::MAX_PENDING;

#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PendingWithdrawals {
    entries: Vec<PendingWithdrawalEntry>,
}

#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
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

    #[inline]
    #[must_use]
    pub fn from_sorted_entries(entries: Vec<(u64, PendingWithdrawal)>) -> Self {
        let mut last_id = None;
        for (id, withdrawal) in &entries {
            if last_id.is_some_and(|last| last >= *id) {
                crate::abort!("pending withdrawal entries must be sorted by unique id");
            }
            if withdrawal.escrow_shares == 0 {
                crate::abort!("pending withdrawal entries must escrow positive shares");
            }
            if withdrawal.epoch_id != EpochId::MIGRATION_INTAKE
                && !withdrawal.epoch_id.is_settlement_epoch()
            {
                crate::abort!("pending withdrawal epoch id is not settlement-eligible");
            }
            last_id = Some(*id);
        }

        Self {
            entries: entries
                .into_iter()
                .map(|(id, withdrawal)| PendingWithdrawalEntry { id, withdrawal })
                .collect(),
        }
    }
}

impl FromIterator<(u64, PendingWithdrawal)> for PendingWithdrawals {
    fn from_iter<T: IntoIterator<Item = (u64, PendingWithdrawal)>>(iter: T) -> Self {
        let mut pending = Self::new();
        for (id, withdrawal) in iter {
            assert!(
                pending.insert(id, withdrawal).is_none(),
                "duplicate pending withdrawal id: {id}"
            );
        }
        pending
    }
}

/// Withdrawal queue storage with FIFO ordering.
///
/// Maintains pending withdrawals keyed by monotonic IDs with escrow parity.
/// The queue uses a sorted `Vec` for efficient iteration and predictable serialization, with two
/// pointers to track the FIFO head and next ID to allocate.
///
/// # Invariants
///
/// - `pending_withdrawals.len() <= max_pending_withdrawals <= MAX_PENDING`
/// - `next_withdraw_to_execute <= next_pending_withdrawal_id`
/// - If `pending_withdrawals.len() > 0`, then `pending_withdrawals` contains `next_withdraw_to_execute`
/// - FIFO withdrawal ordering; no skipping head
/// - `cached_total_escrow == sum(pending_withdrawals.values().map(|w| w.escrow_shares))`
/// - Every entry escrows positive shares and is bound either to a
///   settlement-eligible epoch or to migration intake
#[templar_vault_macros::vault_derive(borsh, borsh_schema, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawQueue {
    /// Pending withdrawals keyed by monotonic ID.
    pending_withdrawals: PendingWithdrawals,
    /// ID of the next withdrawal to execute (queue head).
    pub next_withdraw_to_execute: u64,
    /// Next ID to allocate for new withdrawals (monotonic, never decremented).
    pub next_pending_withdrawal_id: u64,
    /// Cached total of escrow shares across all pending withdrawals.
    /// Maintained incrementally on enqueue/dequeue for O(1) lookups.
    cached_total_escrow: u128,
}

impl Default for WithdrawQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Sum escrow shares across an iterator of pending withdrawals.
fn compute_pending_totals<'a>(iter: impl Iterator<Item = &'a PendingWithdrawal>) -> u128 {
    iter.fold(0u128, |esc, w| esc.saturating_add(w.escrow_shares))
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
        }
    }

    /// Create a queue with initial state (for testing or recovery).
    ///
    /// Used by the storage migration path to re-key legacy fixed-claim
    /// entries into unpriced, migration-intake records: the caller builds
    /// entries exclusively via [`PendingWithdrawal::migrated_legacy`],
    /// which drops the legacy asset quote and pins `min_assets_out` to
    /// zero. The escrow cache is recomputed from the entries.
    #[must_use]
    pub fn with_state<I>(
        pending_withdrawals: I,
        next_withdraw_to_execute: u64,
        next_pending_withdrawal_id: u64,
    ) -> Self
    where
        I: IntoIterator<Item = (u64, PendingWithdrawal)>,
    {
        let pending_withdrawals =
            PendingWithdrawals::from_sorted_entries(pending_withdrawals.into_iter().collect());
        let cached_total_escrow = compute_pending_totals(pending_withdrawals.values());
        Self {
            pending_withdrawals,
            next_withdraw_to_execute,
            next_pending_withdrawal_id,
            cached_total_escrow,
        }
    }

    /// Returns the current queue length.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending_withdrawals.len()
    }

    #[inline]
    #[must_use]
    pub fn pending_withdrawals(&self) -> &PendingWithdrawals {
        &self.pending_withdrawals
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
    /// Constructs an unpriced `PendingWithdrawal` bound to the supplied
    /// settlement epoch and delegates to
    /// [`enqueue_withdrawal`](Self::enqueue_withdrawal). There is no
    /// request-time asset parameter by construction.
    // One request needs the owner, receiver, escrow, minimum, time, epoch,
    // and cap together; splitting them would let a caller enqueue an
    // unpriced request that violates the epoch FIFO law.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue(
        &mut self,
        owner: Address,
        receiver: Address,
        escrow_shares: u128,
        min_assets_out: u128,
        requested_at_ns: TimestampNs,
        epoch_id: EpochId,
        max_pending: u32,
    ) -> Result<u64, QueueError> {
        let withdrawal = PendingWithdrawal::new(
            owner,
            receiver,
            escrow_shares,
            min_assets_out,
            requested_at_ns,
            epoch_id,
        )
        .map_err(QueueError::InvalidRequest)?;
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
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| QueueError::InvariantViolation {
                message: alloc::string::String::from("next_pending_withdrawal_id overflow"),
            })?;

        // Compute cache totals first so we can fail without mutating queue state.
        let new_escrow = self
            .cached_total_escrow
            .checked_add(withdrawal.escrow_shares)
            .ok_or(QueueError::CacheOverflow)?;

        self.pending_withdrawals.insert(id, withdrawal);
        self.next_pending_withdrawal_id = next_id;

        // Update cached totals (overflow already checked)
        self.cached_total_escrow = new_escrow;

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

        self.cached_total_escrow = crate::unwrap_abort!(
            self.cached_total_escrow
                .checked_sub(withdrawal.escrow_shares),
            crate::abort::OVERFLOW,
        );

        // Advance to the next ID in the queue
        self.repair_head_pointer();

        Some((head_id, withdrawal))
    }

    /// Remove a specific pending withdrawal by ID and repair FIFO caches.
    ///
    /// Used by owner cancellation. Revalidates cached escrow totals and moves
    /// the FIFO head to the lowest surviving request ID (or to the next
    /// unallocated ID when the queue empties), so removing a request cannot
    /// leave a stale head pointer or a stale escrow cache behind.
    pub fn remove_pending(&mut self, id: u64) -> Option<PendingWithdrawal> {
        let withdrawal = self.pending_withdrawals.remove(&id)?;

        self.cached_total_escrow = crate::unwrap_abort!(
            self.cached_total_escrow
                .checked_sub(withdrawal.escrow_shares),
            crate::abort::OVERFLOW,
        );

        self.repair_head_pointer();

        Some(withdrawal)
    }

    /// Recompute the FIFO head pointer from surviving request IDs.
    fn repair_head_pointer(&mut self) {
        self.next_withdraw_to_execute = self
            .pending_withdrawals
            .keys()
            .next()
            .copied()
            .unwrap_or(self.next_pending_withdrawal_id);
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

        // Verify cached escrow total matches the recomputed sum.
        let computed_escrow = compute_pending_totals(self.pending_withdrawals.values());
        if self.cached_total_escrow != computed_escrow {
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
    /// Share-denominated only; no asset aggregate can leak a payout
    /// estimate into non-repricing consumers.
    #[inline]
    #[must_use]
    pub fn status(&self) -> QueueStatus {
        QueueStatus {
            length: self.pending_withdrawals.len() as u32,
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

    /// True when any queued entry is a legacy migration-intake record.
    /// Epoch cutoff/settlement flows must keep serving these entries from
    /// the first settled epoch until their FIFO positions clear.
    #[must_use]
    pub fn has_migrated_intake(&self) -> bool {
        self.pending_withdrawals
            .values()
            .any(|w| w.epoch_id == EpochId::MIGRATION_INTAKE)
    }

    /// True when any queued entry was bound to an epoch strictly below
    /// `epoch`. Epoch advancement must fully drain older intake first.
    #[must_use]
    pub fn has_intake_before(&self, epoch: EpochId) -> bool {
        self.pending_withdrawals.values().any(|w| w.epoch_id < epoch)
    }

    /// Recompute the settled claim for the current FIFO head from epoch
    /// state. `None` until settlement covers the head's epoch, which is
    /// exactly when the head must remain queued.
    #[must_use]
    pub fn settled_head_claim(&self, epoch_state: &EpochState) -> Option<u128> {
        let (_, head) = self.head()?;
        settled_claim(head, epoch_state)
    }
}

/// Errors that can occur during queue operations.
#[templar_vault_macros::vault_derive(borsh, serde)]
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
    /// Request failed unpriced-entry validation.
    InvalidRequest(RequestError),
}

#[cfg(all(test, feature = "action-epoch-settlement"))]
mod tests;
