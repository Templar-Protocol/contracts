//! Chain-agnostic escrow types and pure logic functions.
//!
//! This module provides data structures for escrow operations and pure
//! functions for escrow logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).
//!
//! Escrow is share-denominated only. A queued request carries no asset claim:
//! the asset amount a member is owed is derived at execution time from the
//! settled epoch snapshot (`EpochState::settled_claim_for`), never from a
//! quote stored alongside the escrowed shares.

use crate::types::{Address, TimestampNs};

pub use crate::types::EscrowSettlement;

/// Escrow entry for a single actor.
///
/// Tracks shares held in escrow for a pending withdrawal. There is no
/// asset-denominated field, so a request-time payout estimate cannot be
/// stored, read, or consumed by any escrow path.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct EscrowEntry {
    pub owner: Address,
    pub shares: u128,
    pub created_at_ns: TimestampNs,
}

impl EscrowEntry {
    #[inline]
    #[must_use]
    pub fn new(owner: Address, shares: u128, created_at_ns: TimestampNs) -> Self {
        Self {
            owner,
            shares,
            created_at_ns,
        }
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shares == 0
    }
}

/// Result of applying a settlement to an escrow entry.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct SettlementResult {
    pub burned: u128,
    pub refunded: u128,
}

/// Aggregate escrow statistics.
///
/// Deliberately share-denominated: an aggregate asset figure over escrow
/// entries is the fixed-claim class this model removes.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EscrowStats {
    pub count: u32,
    pub total_shares: u128,
}

/// Validate and price an escrow settlement for an escrow entry.
///
/// Financial law: a settlement must consume escrow exactly. The checked sum of
/// burned and refunded shares must equal the escrowed shares of the entry, so
/// an escrowed share can neither be stranded in escrow nor settled twice.
/// Returns `None` on any mismatch or checked-add overflow, leaving the caller
/// free of partially applied state.
///
/// The helpers here are pure arithmetic validators: they price a settlement
/// against an entry without mutating it. Consumption is enforced by the
/// request lifecycle: settlement removes the request from the withdrawal
/// queue and repairs the FIFO/escrow caches, so a settled request can never
/// be settled or cancelled a second time.
#[must_use]
pub fn apply_settlement(
    entry: &EscrowEntry,
    settlement: &EscrowSettlement,
) -> Option<SettlementResult> {
    if !can_apply_settlement(entry, settlement) {
        return None;
    }

    Some(SettlementResult {
        burned: settlement.to_burn,
        refunded: settlement.refund,
    })
}

/// Validate that a settlement consumes an escrow entry exactly.
///
/// True only when `to_burn + refund` (checked) equals the escrowed shares.
#[inline]
#[must_use]
pub fn can_apply_settlement(entry: &EscrowEntry, settlement: &EscrowSettlement) -> bool {
    settlement
        .to_burn
        .checked_add(settlement.refund)
        .is_some_and(|total| total == entry.shares)
}

/// Check if an escrow entry is stale (past its expected settlement time).
#[inline]
#[must_use]
pub fn is_stale(entry: &EscrowEntry, now_ns: TimestampNs, max_age_ns: u64) -> bool {
    now_ns > entry.created_at_ns.saturating_add_u64(max_age_ns)
}

/// Compute aggregate escrow statistics from an iterator of entries.
#[must_use]
pub fn compute_escrow_stats<'a, I>(entries: I) -> EscrowStats
where
    I: IntoIterator<Item = &'a EscrowEntry>,
{
    let mut stats = EscrowStats::default();

    for entry in entries {
        stats.count = stats.count.saturating_add(1);
        stats.total_shares = stats.total_shares.saturating_add(entry.shares);
    }

    stats
}

/// Find an escrow entry by owner.
#[must_use]
pub fn find_by_owner<'a, I>(entries: I, owner: &Address) -> Option<&'a EscrowEntry>
where
    I: IntoIterator<Item = &'a EscrowEntry>,
{
    entries.into_iter().find(|e| &e.owner == owner)
}

/// Calculate total shares that would be burned across multiple settlements.
///
/// Returns `None` when the checked total overflows, so a batch cannot be
/// approved on a saturated burn figure.
#[must_use]
pub fn total_burn<'a, I>(settlements: I) -> Option<u128>
where
    I: IntoIterator<Item = &'a EscrowSettlement>,
{
    settlements
        .into_iter()
        .try_fold(0u128, |acc, s| acc.checked_add(s.to_burn))
}

/// Calculate total shares that would be refunded across multiple settlements.
///
/// Returns `None` when the checked total overflows, so a batch cannot be
/// approved on a saturated refund figure.
#[must_use]
pub fn total_refund<'a, I>(settlements: I) -> Option<u128>
where
    I: IntoIterator<Item = &'a EscrowSettlement>,
{
    settlements
        .into_iter()
        .try_fold(0u128, |acc, s| acc.checked_add(s.refund))
}

#[cfg(all(test, feature = "action-epoch-settlement"))]
mod tests;

