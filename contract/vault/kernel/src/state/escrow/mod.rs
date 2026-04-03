//! Chain-agnostic escrow types and pure logic functions.
//!
//! This module provides data structures for escrow operations and pure
//! functions for escrow logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

use crate::math::number::Number;
use crate::types::{Address, TimestampNs};

pub use crate::types::EscrowSettlement;

/// Escrow entry for a single actor.
///
/// Tracks shares held in escrow for a pending withdrawal.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct EscrowEntry {
    pub owner: Address,
    pub shares: u128,
    pub created_at_ns: TimestampNs,
    pub expected_assets: u128,
}

impl EscrowEntry {
    #[inline]
    #[must_use]
    pub fn new(
        owner: Address,
        shares: u128,
        created_at_ns: TimestampNs,
        expected_assets: u128,
    ) -> Self {
        Self {
            owner,
            shares,
            created_at_ns,
            expected_assets,
        }
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shares == 0
    }
}

/// Result of applying a settlement to an escrow entry.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub struct SettlementResult {
    pub burned: u128,
    pub refunded: u128,
    pub remaining: u128,
}

/// Aggregate escrow statistics.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EscrowStats {
    pub count: u32,
    pub total_shares: u128,
    pub total_expected_assets: u128,
}

/// Apply an escrow settlement to an escrow entry.
#[must_use]
pub fn apply_settlement(
    entry: &EscrowEntry,
    settlement: &EscrowSettlement,
) -> Option<SettlementResult> {
    let total_settled = settlement.to_burn.checked_add(settlement.refund)?;

    if total_settled > entry.shares {
        return None;
    }

    let remaining = entry.shares.saturating_sub(total_settled);

    Some(SettlementResult {
        burned: settlement.to_burn,
        refunded: settlement.refund,
        remaining,
    })
}

/// Compute a proportional settlement based on actual vs expected assets.
#[must_use]
pub fn settle_proportional(entry: &EscrowEntry, actual_assets: u128) -> EscrowSettlement {
    settle_proportional_raw(entry.shares, entry.expected_assets, actual_assets)
}

/// Compute a proportional settlement directly from escrow share and asset amounts.
#[must_use]
pub fn settle_proportional_raw(
    shares: u128,
    expected_assets: u128,
    actual_assets: u128,
) -> EscrowSettlement {
    if shares == 0 {
        return EscrowSettlement {
            to_burn: 0,
            refund: 0,
        };
    }

    if actual_assets == 0 {
        return EscrowSettlement::refund_all(shares);
    }

    if expected_assets == 0 {
        return EscrowSettlement::refund_all(shares);
    }

    if actual_assets >= expected_assets {
        return EscrowSettlement::burn_all(shares);
    }

    // Proportional: burn shares proportional to actual/expected ratio.
    // Use ceil to avoid zero-burn partials (assets out without burning shares).
    let to_burn = Number::mul_div_ceil(
        Number::from(shares),
        Number::from(actual_assets),
        Number::from(expected_assets),
    )
    .as_u128_trunc();

    let refund = shares.saturating_sub(to_burn);

    EscrowSettlement::partial(to_burn, refund)
}

/// Validate that an escrow entry has sufficient shares for a settlement.
#[inline]
#[must_use]
pub fn can_apply_settlement(entry: &EscrowEntry, settlement: &EscrowSettlement) -> bool {
    settlement
        .to_burn
        .checked_add(settlement.refund)
        .is_some_and(|total| total <= entry.shares)
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
        stats.total_expected_assets = stats
            .total_expected_assets
            .saturating_add(entry.expected_assets);
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
#[must_use]
pub fn total_burn<'a, I>(settlements: I) -> u128
where
    I: IntoIterator<Item = &'a EscrowSettlement>,
{
    settlements
        .into_iter()
        .map(|s| s.to_burn)
        .fold(0u128, |acc, x| acc.saturating_add(x))
}

/// Calculate total shares that would be refunded across multiple settlements.
#[must_use]
pub fn total_refund<'a, I>(settlements: I) -> u128
where
    I: IntoIterator<Item = &'a EscrowSettlement>,
{
    settlements
        .into_iter()
        .map(|s| s.refund)
        .fold(0u128, |acc, x| acc.saturating_add(x))
}

#[cfg(test)]
mod tests;
