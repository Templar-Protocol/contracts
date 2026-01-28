//! Chain-agnostic escrow types and pure logic functions.
//!
//! This module provides data structures for escrow operations and pure
//! functions for escrow logic. Storage implementation is left to chain-specific
//! executors (NEAR, Soroban, etc.).

#[cfg(feature = "near")]
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "near")]
use serde::{Deserialize, Serialize};

use crate::math::number::Number;
use crate::types::{ActorId, TimestampNs};

// Re-export EscrowSettlement from types module
pub use crate::types::EscrowSettlement;

// ============================================================================
// Types
// ============================================================================

/// Escrow entry for a single actor.
///
/// Tracks shares held in escrow for a pending withdrawal.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowEntry {
    /// Actor whose shares are escrowed.
    pub owner: ActorId,
    /// Number of shares held in escrow.
    pub shares: u128,
    /// Timestamp when escrow was created.
    pub created_at_ns: TimestampNs,
    /// Expected assets at time of escrow creation (for slippage protection).
    pub expected_assets: u128,
}

impl EscrowEntry {
    /// Create a new escrow entry.
    #[inline]
    #[must_use]
    pub fn new(
        owner: ActorId,
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

    /// Check if this escrow entry is empty (zero shares).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shares == 0
    }
}

/// Result of applying a settlement to an escrow entry.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementResult {
    /// Shares actually burned from escrow.
    pub burned: u128,
    /// Shares refunded to the owner.
    pub refunded: u128,
    /// Remaining shares in escrow (should be 0 if fully settled).
    pub remaining: u128,
}

/// Aggregate escrow statistics.
#[cfg_attr(feature = "near", derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EscrowStats {
    /// Total number of escrow entries.
    pub count: u32,
    /// Total shares held in escrow.
    pub total_shares: u128,
    /// Total expected assets across all escrows.
    pub total_expected_assets: u128,
}

// ============================================================================
// Pure Functions - Settlement
// ============================================================================

/// Apply an escrow settlement to an escrow entry.
///
/// Validates that the settlement does not exceed available escrowed shares.
///
/// # Arguments
/// * `entry` - The escrow entry to settle.
/// * `settlement` - The settlement to apply.
///
/// # Returns
/// `Some(SettlementResult)` if valid, `None` if settlement exceeds escrow.
#[must_use]
pub fn apply_settlement(entry: &EscrowEntry, settlement: &EscrowSettlement) -> Option<SettlementResult> {
    let total_settled = settlement.to_burn.saturating_add(settlement.refund);

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

/// Compute a full settlement that burns all escrowed shares.
///
/// # Arguments
/// * `entry` - The escrow entry to fully settle.
///
/// # Returns
/// `EscrowSettlement` that burns all shares.
#[inline]
#[must_use]
pub fn settle_full_burn(entry: &EscrowEntry) -> EscrowSettlement {
    EscrowSettlement::burn_all(entry.shares)
}

/// Compute a full settlement that refunds all escrowed shares.
///
/// # Arguments
/// * `entry` - The escrow entry to fully refund.
///
/// # Returns
/// `EscrowSettlement` that refunds all shares.
#[inline]
#[must_use]
pub fn settle_full_refund(entry: &EscrowEntry) -> EscrowSettlement {
    EscrowSettlement::refund_all(entry.shares)
}

/// Compute a proportional settlement based on actual vs expected assets.
///
/// # Arguments
/// * `entry` - The escrow entry to settle.
/// * `actual_assets` - Assets actually available for withdrawal.
///
/// # Returns
/// `EscrowSettlement` with proportional burn and refund.
#[must_use]
pub fn settle_proportional(entry: &EscrowEntry, actual_assets: u128) -> EscrowSettlement {
    if entry.shares == 0 {
        return EscrowSettlement {
            to_burn: 0,
            refund: 0,
        };
    }

    if actual_assets == 0 {
        return EscrowSettlement::refund_all(entry.shares);
    }

    if actual_assets >= entry.expected_assets || entry.expected_assets == 0 {
        return EscrowSettlement::burn_all(entry.shares);
    }

    // Proportional: burn shares proportional to actual/expected ratio
    let to_burn = Number::mul_div_floor(
        Number::from(entry.shares),
        Number::from(actual_assets),
        Number::from(entry.expected_assets),
    )
    .as_u128_trunc();

    let refund = entry.shares.saturating_sub(to_burn);

    EscrowSettlement::partial(to_burn, refund)
}

// ============================================================================
// Pure Functions - Validation
// ============================================================================

/// Validate that an escrow entry has sufficient shares for a settlement.
///
/// # Arguments
/// * `entry` - The escrow entry to validate against.
/// * `settlement` - The settlement to validate.
///
/// # Returns
/// `true` if the settlement can be applied.
#[inline]
#[must_use]
pub fn can_apply_settlement(entry: &EscrowEntry, settlement: &EscrowSettlement) -> bool {
    let total = settlement.to_burn.saturating_add(settlement.refund);
    total <= entry.shares
}

/// Check if an escrow entry is stale (past its expected settlement time).
///
/// # Arguments
/// * `entry` - The escrow entry to check.
/// * `now_ns` - Current timestamp in nanoseconds.
/// * `max_age_ns` - Maximum age before an escrow is considered stale.
///
/// # Returns
/// `true` if the escrow is older than `max_age_ns`.
#[inline]
#[must_use]
pub fn is_stale(entry: &EscrowEntry, now_ns: TimestampNs, max_age_ns: u64) -> bool {
    now_ns > entry.created_at_ns.saturating_add(max_age_ns)
}

// ============================================================================
// Pure Functions - Aggregation
// ============================================================================

/// Compute aggregate escrow statistics from an iterator of entries.
///
/// # Arguments
/// * `entries` - Iterator over escrow entries.
///
/// # Returns
/// `EscrowStats` with totals.
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
///
/// # Arguments
/// * `entries` - Iterator over escrow entries.
/// * `owner` - The owner to search for.
///
/// # Returns
/// `Some(&EscrowEntry)` if found, `None` otherwise.
#[must_use]
pub fn find_by_owner<'a, I>(entries: I, owner: &ActorId) -> Option<&'a EscrowEntry>
where
    I: IntoIterator<Item = &'a EscrowEntry>,
{
    entries.into_iter().find(|e| &e.owner == owner)
}

/// Calculate total shares that would be burned across multiple settlements.
///
/// # Arguments
/// * `settlements` - Iterator over settlements.
///
/// # Returns
/// Total shares to burn.
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
///
/// # Arguments
/// * `settlements` - Iterator over settlements.
///
/// # Returns
/// Total shares to refund.
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    fn make_entry(owner: &str, shares: u128, expected: u128) -> EscrowEntry {
        EscrowEntry::new(
            owner.to_string(),
            shares,
            1_000_000_000_000, // 1 second in ns
            expected,
        )
    }

    #[test]
    fn test_escrow_entry_is_empty() {
        let entry = make_entry("alice", 0, 1000);
        assert!(entry.is_empty());

        let entry = make_entry("alice", 100, 1000);
        assert!(!entry.is_empty());
    }

    #[test]
    fn test_apply_settlement_valid() {
        let entry = make_entry("alice", 100, 1000);
        let settlement = EscrowSettlement::partial(60, 40);

        let result = apply_settlement(&entry, &settlement);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.burned, 60);
        assert_eq!(result.refunded, 40);
        assert_eq!(result.remaining, 0);
    }

    #[test]
    fn test_apply_settlement_partial() {
        let entry = make_entry("alice", 100, 1000);
        let settlement = EscrowSettlement::partial(30, 20);

        let result = apply_settlement(&entry, &settlement);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.burned, 30);
        assert_eq!(result.refunded, 20);
        assert_eq!(result.remaining, 50);
    }

    #[test]
    fn test_apply_settlement_exceeds_escrow() {
        let entry = make_entry("alice", 100, 1000);
        let settlement = EscrowSettlement::partial(80, 30); // 110 > 100

        let result = apply_settlement(&entry, &settlement);
        assert!(result.is_none());
    }

    #[test]
    fn test_settle_full_burn() {
        let entry = make_entry("alice", 100, 1000);
        let settlement = settle_full_burn(&entry);

        assert_eq!(settlement.to_burn, 100);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_settle_full_refund() {
        let entry = make_entry("alice", 100, 1000);
        let settlement = settle_full_refund(&entry);

        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 100);
    }

    #[test]
    fn test_settle_proportional_full() {
        let entry = make_entry("alice", 100, 1000);

        // Full assets available
        let settlement = settle_proportional(&entry, 1000);
        assert_eq!(settlement.to_burn, 100);
        assert_eq!(settlement.refund, 0);

        // More than expected
        let settlement = settle_proportional(&entry, 2000);
        assert_eq!(settlement.to_burn, 100);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_settle_proportional_zero() {
        let entry = make_entry("alice", 100, 1000);

        let settlement = settle_proportional(&entry, 0);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 100);
    }

    #[test]
    fn test_settle_proportional_partial() {
        let entry = make_entry("alice", 100, 1000);

        // 50% available
        let settlement = settle_proportional(&entry, 500);
        assert_eq!(settlement.to_burn, 50);
        assert_eq!(settlement.refund, 50);

        // 75% available
        let settlement = settle_proportional(&entry, 750);
        assert_eq!(settlement.to_burn, 75);
        assert_eq!(settlement.refund, 25);
    }

    #[test]
    fn test_can_apply_settlement() {
        let entry = make_entry("alice", 100, 1000);

        // Valid settlement
        assert!(can_apply_settlement(&entry, &EscrowSettlement::partial(50, 50)));
        assert!(can_apply_settlement(&entry, &EscrowSettlement::burn_all(100)));
        assert!(can_apply_settlement(&entry, &EscrowSettlement::refund_all(100)));

        // Invalid settlement (exceeds shares)
        assert!(!can_apply_settlement(&entry, &EscrowSettlement::partial(60, 50)));
        assert!(!can_apply_settlement(&entry, &EscrowSettlement::burn_all(101)));
    }

    #[test]
    fn test_is_stale() {
        let entry = make_entry("alice", 100, 1000);
        let max_age = 60_000_000_000u64; // 60 seconds

        // Not stale
        assert!(!is_stale(&entry, 1_000_000_000_000, max_age));
        assert!(!is_stale(&entry, 1_060_000_000_000, max_age));

        // Stale
        assert!(is_stale(&entry, 1_060_000_000_001, max_age));
        assert!(is_stale(&entry, 2_000_000_000_000, max_age));
    }

    #[test]
    fn test_compute_escrow_stats() {
        let entries: Vec<EscrowEntry> = vec![
            make_entry("alice", 100, 1000),
            make_entry("bob", 200, 2000),
            make_entry("charlie", 300, 3000),
        ];

        let stats = compute_escrow_stats(&entries);
        assert_eq!(stats.count, 3);
        assert_eq!(stats.total_shares, 600);
        assert_eq!(stats.total_expected_assets, 6000);
    }

    #[test]
    fn test_find_by_owner() {
        let entries: Vec<EscrowEntry> = vec![
            make_entry("alice", 100, 1000),
            make_entry("bob", 200, 2000),
        ];

        let found = find_by_owner(&entries, &"bob".to_string());
        assert!(found.is_some());
        assert_eq!(found.unwrap().shares, 200);

        let not_found = find_by_owner(&entries, &"charlie".to_string());
        assert!(not_found.is_none());
    }

    #[test]
    fn test_total_burn_and_refund() {
        let settlements: Vec<EscrowSettlement> = vec![
            EscrowSettlement::partial(50, 10),
            EscrowSettlement::partial(30, 20),
            EscrowSettlement::burn_all(100),
        ];

        assert_eq!(total_burn(&settlements), 180);
        assert_eq!(total_refund(&settlements), 30);
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

    /// Strategy for generating an EscrowEntry
    fn arb_entry() -> impl Strategy<Value = EscrowEntry> {
        (
            1u32..1000u32,       // owner index
            0u128..=u64::MAX as u128,  // shares
            0u64..u64::MAX,      // created_at
            0u128..=u64::MAX as u128,  // expected_assets
        )
            .prop_map(|(owner_idx, shares, ts, expected)| {
                EscrowEntry::new(
                    format!("owner_{}", owner_idx),
                    shares,
                    ts,
                    expected,
                )
            })
    }

    /// Strategy for generating a list of EscrowEntry
    fn arb_entries(max_len: usize) -> impl Strategy<Value = Vec<EscrowEntry>> {
        proptest::collection::vec(arb_entry(), 0..=max_len)
    }

    proptest! {
        // ===================================================================
        // Property: settle_proportional burn + refund == shares
        // Invariant: Total settled equals original shares
        // ===================================================================
        #[test]
        fn settle_proportional_conserves_shares(
            shares in 0u128..=u64::MAX as u128,
            expected_assets in 1u128..=u64::MAX as u128,
            actual_assets in 0u128..=u64::MAX as u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                expected_assets,
            );
            let settlement = settle_proportional(&entry, actual_assets);
            let total = settlement.to_burn.saturating_add(settlement.refund);
            prop_assert_eq!(total, shares);
        }

        // ===================================================================
        // Property: settle_proportional full burn when actual >= expected
        // Invariant: Burns all when redemption meets expectation
        // ===================================================================
        #[test]
        fn settle_proportional_full_burn(
            shares in 1u128..=u64::MAX as u128,
            expected_assets in 1u128..=u64::MAX as u128,
            extra in 0u128..=1_000_000u128,
        ) {
            let actual_assets = expected_assets.saturating_add(extra);
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                expected_assets,
            );
            let settlement = settle_proportional(&entry, actual_assets);

            prop_assert_eq!(settlement.to_burn, shares);
            prop_assert_eq!(settlement.refund, 0);
        }

        // ===================================================================
        // Property: settle_proportional full refund when actual == 0
        // Invariant: Refunds all on cancellation
        // ===================================================================
        #[test]
        fn settle_proportional_full_refund(
            shares in 1u128..=u64::MAX as u128,
            expected_assets in 1u128..=u64::MAX as u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                expected_assets,
            );
            let settlement = settle_proportional(&entry, 0);

            prop_assert_eq!(settlement.to_burn, 0);
            prop_assert_eq!(settlement.refund, shares);
        }

        // ===================================================================
        // Property: settle_full_burn burns all
        // Invariant: settle_full_burn(entry) burns all shares
        // ===================================================================
        #[test]
        fn settle_full_burn_burns_all(
            shares in 0u128..=u64::MAX as u128,
            expected_assets in 0u128..=u64::MAX as u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                expected_assets,
            );
            let settlement = settle_full_burn(&entry);

            prop_assert_eq!(settlement.to_burn, shares);
            prop_assert_eq!(settlement.refund, 0);
        }

        // ===================================================================
        // Property: settle_full_refund refunds all
        // Invariant: settle_full_refund(entry) refunds all shares
        // ===================================================================
        #[test]
        fn settle_full_refund_refunds_all(
            shares in 0u128..=u64::MAX as u128,
            expected_assets in 0u128..=u64::MAX as u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                expected_assets,
            );
            let settlement = settle_full_refund(&entry);

            prop_assert_eq!(settlement.to_burn, 0);
            prop_assert_eq!(settlement.refund, shares);
        }

        // ===================================================================
        // Property: apply_settlement succeeds when total <= shares
        // Invariant: Valid settlements are applied
        // ===================================================================
        #[test]
        fn apply_settlement_valid(
            shares in 1u128..=u64::MAX as u128,
            burn_ratio in 0u8..=100u8,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                1000,
            );
            let to_burn = (shares as u128 * burn_ratio as u128) / 100;
            let refund = shares - to_burn;
            let settlement = EscrowSettlement::partial(to_burn, refund);

            let result = apply_settlement(&entry, &settlement);
            prop_assert!(result.is_some());

            let result = result.unwrap();
            prop_assert_eq!(result.burned, to_burn);
            prop_assert_eq!(result.refunded, refund);
            prop_assert_eq!(result.remaining, 0);
        }

        // ===================================================================
        // Property: apply_settlement fails when total > shares
        // Invariant: Invalid settlements are rejected
        // ===================================================================
        #[test]
        fn apply_settlement_invalid(
            shares in 1u128..=u64::MAX as u128 - 1,
            excess in 1u128..=1_000_000u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                1000,
            );
            let settlement = EscrowSettlement::partial(shares, excess);

            let result = apply_settlement(&entry, &settlement);
            prop_assert!(result.is_none());
        }

        // ===================================================================
        // Property: can_apply_settlement consistency
        // Invariant: can_apply iff total <= shares
        // ===================================================================
        #[test]
        fn can_apply_settlement_consistency(
            shares in 0u128..=u64::MAX as u128,
            to_burn in 0u128..=u64::MAX as u128 / 2,
            refund in 0u128..=u64::MAX as u128 / 2,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                1000,
            );
            let settlement = EscrowSettlement::partial(to_burn, refund);
            let total = to_burn.saturating_add(refund);

            let can = can_apply_settlement(&entry, &settlement);
            prop_assert_eq!(can, total <= shares);
        }

        // ===================================================================
        // Property: is_stale consistency
        // Invariant: stale iff now > created_at + max_age
        // ===================================================================
        #[test]
        fn is_stale_consistency(
            created_at in 0u64..=u64::MAX / 2,
            max_age in 0u64..=u64::MAX / 4,
            delta in 0u64..=u64::MAX / 4,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                100,
                created_at,
                1000,
            );
            let now = created_at.saturating_add(delta);
            let threshold = created_at.saturating_add(max_age);

            let stale = is_stale(&entry, now, max_age);
            prop_assert_eq!(stale, now > threshold);
        }

        // ===================================================================
        // Property: compute_escrow_stats totals are correct
        // Invariant: Stats match manual sums
        // ===================================================================
        #[test]
        fn compute_escrow_stats_correct(
            entries in arb_entries(20),
        ) {
            let stats = compute_escrow_stats(&entries);

            let expected_count = entries.len() as u32;
            let expected_shares: u128 = entries.iter().map(|e| e.shares).sum();
            let expected_assets: u128 = entries.iter().map(|e| e.expected_assets).sum();

            prop_assert_eq!(stats.count, expected_count);
            prop_assert_eq!(stats.total_shares, expected_shares);
            prop_assert_eq!(stats.total_expected_assets, expected_assets);
        }

        // ===================================================================
        // Property: total_burn equals sum of to_burn
        // Invariant: Aggregation is correct
        // ===================================================================
        #[test]
        fn total_burn_correct(
            settlements in proptest::collection::vec(
                (0u128..=u64::MAX as u128, 0u128..=u64::MAX as u128)
                    .prop_map(|(b, r)| EscrowSettlement::partial(b, r)),
                0..20
            ),
        ) {
            let result = total_burn(&settlements);
            let expected: u128 = settlements.iter().map(|s| s.to_burn).sum();
            prop_assert_eq!(result, expected);
        }

        // ===================================================================
        // Property: total_refund equals sum of refund
        // Invariant: Aggregation is correct
        // ===================================================================
        #[test]
        fn total_refund_correct(
            settlements in proptest::collection::vec(
                (0u128..=u64::MAX as u128, 0u128..=u64::MAX as u128)
                    .prop_map(|(b, r)| EscrowSettlement::partial(b, r)),
                0..20
            ),
        ) {
            let result = total_refund(&settlements);
            let expected: u128 = settlements.iter().map(|s| s.refund).sum();
            prop_assert_eq!(result, expected);
        }

        // ===================================================================
        // Property: EscrowEntry::is_empty consistency
        // Invariant: empty iff shares == 0
        // ===================================================================
        #[test]
        fn entry_is_empty_consistency(
            shares in 0u128..=u64::MAX as u128,
        ) {
            let entry = EscrowEntry::new(
                "owner".to_string(),
                shares,
                0,
                1000,
            );
            prop_assert_eq!(entry.is_empty(), shares == 0);
        }

        // ===================================================================
        // Property: EscrowSettlement::burn_all consistency
        // Invariant: burn_all(x) == partial(x, 0)
        // ===================================================================
        #[test]
        fn burn_all_consistency(shares in 0u128..=u64::MAX as u128) {
            let s1 = EscrowSettlement::burn_all(shares);
            let s2 = EscrowSettlement::partial(shares, 0);
            prop_assert_eq!(s1.to_burn, s2.to_burn);
            prop_assert_eq!(s1.refund, s2.refund);
        }

        // ===================================================================
        // Property: EscrowSettlement::refund_all consistency
        // Invariant: refund_all(x) == partial(0, x)
        // ===================================================================
        #[test]
        fn refund_all_consistency(shares in 0u128..=u64::MAX as u128) {
            let s1 = EscrowSettlement::refund_all(shares);
            let s2 = EscrowSettlement::partial(0, shares);
            prop_assert_eq!(s1.to_burn, s2.to_burn);
            prop_assert_eq!(s1.refund, s2.refund);
        }
    }
}
