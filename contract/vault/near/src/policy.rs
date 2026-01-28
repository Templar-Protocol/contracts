//! Policy module bridging curator-primitives with NEAR vault types.
//!
//! This module provides:
//! - Type conversions between common/near types and curator-primitives types
//! - Re-exports of curator-primitives pure functions for policy enforcement
//! - NEAR-specific wrappers where needed

use templar_common::vault::{CapGroupRecord as CommonCapGroupRecord, MarketId};

// Re-export curator-primitives types for external consumers
pub use templar_curator_primitives::policy::{
    cap_group::{
        can_allocate_to_group, compute_available_capacity, compute_effective_cap, enforce_cap_group,
        CapGroup, CapGroupError, CapGroupId as PrimitiveCapGroupId,
        CapGroupRecord as PrimitiveCapGroupRecord,
    },
    market_lock::{
        acquire_lock, cleanup_expired_locks, clear_all_locks, find_locked_targets,
        get_locked_targets, is_locked_by_op, is_market_locked, release_all_by_op, release_lock,
        release_lock_by_op, MarketLock, MarketLockSet,
    },
    supply_queue::{
        compute_queue_total, compute_queue_totals_by_target, dequeue_supply, drain_queue,
        enqueue_supply, remove_target_entries, to_allocation_plan, SupplyQueue, SupplyQueueEntry,
        SupplyQueueError,
    },
    withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity, compute_available_liquidity,
        compute_route_total, to_withdrawal_plan, validate_withdraw_route, WithdrawRoute,
        WithdrawRouteEntry, WithdrawRouteError,
    },
};

/// Convert a common CapGroupRecord to a curator-primitives CapGroup for use with pure functions.
///
/// The common module stores cap and relative_cap directly on the record,
/// while curator-primitives separates them into a CapGroup struct.
pub fn to_primitive_cap_group(record: &CommonCapGroupRecord) -> CapGroup {
    CapGroup {
        absolute_cap: record.cap.0,
        relative_cap: record.relative_cap,
    }
}

/// Convert a common CapGroupRecord to a curator-primitives CapGroupRecord.
pub fn to_primitive_cap_group_record(record: &CommonCapGroupRecord) -> PrimitiveCapGroupRecord {
    PrimitiveCapGroupRecord {
        cap: to_primitive_cap_group(record),
        principal: record.principal,
    }
}

/// Check if an allocation is allowed for a cap group using common types.
///
/// This is a convenience wrapper around the curator-primitives `can_allocate_to_group` function.
pub fn can_allocate_to_common_cap_group(
    record: &CommonCapGroupRecord,
    amount: u128,
    total_assets: u128,
) -> bool {
    let cap = to_primitive_cap_group(record);
    can_allocate_to_group(&cap, record.principal, amount, total_assets)
}

/// Enforce cap group constraints using common types.
///
/// This is a convenience wrapper around the curator-primitives `enforce_cap_group` function.
pub fn enforce_common_cap_group(
    record: &CommonCapGroupRecord,
    amount: u128,
    total_assets: u128,
) -> Result<(), CapGroupError> {
    let cap = to_primitive_cap_group(record);
    enforce_cap_group(&cap, record.principal, amount, total_assets)
}

/// Compute the effective cap for a common CapGroupRecord.
///
/// Note: This preserves NEAR vault semantics where `relative_cap >= 100%` (Wad::one())
/// means "no relative cap constraint, use absolute cap". This differs from the general
/// curator-primitives behavior which would compute 100% of total_assets.
pub fn compute_effective_cap_for_common(record: &CommonCapGroupRecord, total_assets: u128) -> u128 {
    use templar_common::vault::wad::Wad;

    // NEAR vault semantics: relative_cap >= 100% means no relative cap constraint
    let rel_cap = record.relative_cap.min(Wad::one());
    if rel_cap >= Wad::one() {
        return record.cap.0;
    }

    // Use curator-primitives for the general case
    let cap = to_primitive_cap_group(record);
    compute_effective_cap(&cap, total_assets)
}

/// Compute available capacity for a common CapGroupRecord.
///
/// Note: This preserves NEAR vault semantics where `relative_cap >= 100%` (Wad::one())
/// means "no relative cap constraint, use absolute cap". This differs from the general
/// curator-primitives behavior which would compute 100% of total_assets.
pub fn compute_available_capacity_for_common(
    record: &CommonCapGroupRecord,
    total_assets: u128,
) -> u128 {
    let effective_cap = compute_effective_cap_for_common(record, total_assets);
    effective_cap.saturating_sub(record.principal)
}

/// Validate a supply queue represented as a Vec<MarketId>.
///
/// The NEAR vault uses a simple Vec<MarketId> for its supply queue,
/// while curator-primitives uses a more detailed SupplyQueueEntry structure.
/// This function validates the basic requirements: no duplicates.
pub fn validate_supply_queue_no_duplicates(queue: &[MarketId]) -> bool {
    let mut seen = std::collections::HashSet::new();
    for m in queue {
        if !seen.insert(m.0) {
            return false;
        }
    }
    true
}

/// Build a withdraw route from market principals.
///
/// Converts NEAR MarketId to TargetId for use with curator-primitives,
/// then returns the validated route.
pub fn build_withdraw_route_from_markets(
    principals: &[(MarketId, u128)],
    target_amount: u128,
) -> Result<Vec<(MarketId, u128)>, WithdrawRouteError> {
    // Convert to TargetId (u32) for curator-primitives
    let target_principals: Vec<(u32, u128)> = principals.iter().map(|(m, p)| (m.0, *p)).collect();

    let route = build_withdraw_route(&target_principals, target_amount)?;

    // Convert back to MarketId
    Ok(route
        .entries
        .iter()
        .map(|e| (MarketId(e.target_id), e.max_amount))
        .collect())
}

/// Validate a withdraw route represented as Vec<MarketId>.
///
/// Checks for duplicates in the route.
pub fn validate_withdraw_route_no_duplicates(route: &[MarketId]) -> bool {
    let mut seen = std::collections::HashSet::new();
    for m in route {
        if !seen.insert(m.0) {
            return false;
        }
    }
    true
}

/// Check if any markets in the list are locked.
///
/// Converts MarketId to TargetId for use with curator-primitives.
pub fn find_locked_markets(
    lock_set: &MarketLockSet,
    markets: &[MarketId],
    current_ns: u64,
) -> Vec<MarketId> {
    let targets: Vec<u32> = markets.iter().map(|m| m.0).collect();
    let locked = find_locked_targets(lock_set, &targets, current_ns);
    locked.into_iter().map(MarketId).collect()
}

/// Check if a specific market is locked.
pub fn is_market_id_locked(lock_set: &MarketLockSet, market: MarketId, current_ns: u64) -> bool {
    is_market_locked(lock_set, market.0, current_ns)
}

/// Get all locked market IDs.
pub fn get_locked_market_ids(lock_set: &MarketLockSet, current_ns: u64) -> Vec<MarketId> {
    get_locked_targets(lock_set, current_ns)
        .into_iter()
        .map(MarketId)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::json_types::U128;
    use templar_common::vault::wad::Wad;

    const WAD: u128 = 1_000_000_000_000_000_000_000_000;

    #[test]
    fn test_to_primitive_cap_group() {
        let record = CommonCapGroupRecord {
            cap: U128(1000),
            relative_cap: Wad::from(WAD / 2), // 50%
            principal: 300,
        };

        let cap = to_primitive_cap_group(&record);
        assert_eq!(cap.absolute_cap, 1000);
        assert_eq!(cap.relative_cap, Wad::from(WAD / 2));
    }

    #[test]
    fn test_can_allocate_to_common_cap_group() {
        let record = CommonCapGroupRecord {
            cap: U128(1000),
            relative_cap: Wad::one(), // 100%
            principal: 300,
        };

        // Should be able to allocate 500 more (300 + 500 = 800 < 1000)
        assert!(can_allocate_to_common_cap_group(&record, 500, 2000));

        // Should not be able to allocate 800 more (300 + 800 = 1100 > 1000)
        assert!(!can_allocate_to_common_cap_group(&record, 800, 2000));
    }

    #[test]
    fn test_enforce_common_cap_group() {
        let record = CommonCapGroupRecord {
            cap: U128(1000),
            relative_cap: Wad::one(),
            principal: 300,
        };

        // Valid allocation
        assert!(enforce_common_cap_group(&record, 500, 2000).is_ok());

        // Invalid allocation
        let result = enforce_common_cap_group(&record, 800, 2000);
        assert!(matches!(
            result,
            Err(CapGroupError::ExceedsAbsoluteCap { .. })
        ));
    }

    #[test]
    fn test_validate_supply_queue_no_duplicates() {
        let queue_ok = vec![MarketId(1), MarketId(2), MarketId(3)];
        assert!(validate_supply_queue_no_duplicates(&queue_ok));

        let queue_dup = vec![MarketId(1), MarketId(2), MarketId(1)];
        assert!(!validate_supply_queue_no_duplicates(&queue_dup));
    }

    #[test]
    fn test_validate_withdraw_route_no_duplicates() {
        let route_ok = vec![MarketId(1), MarketId(2), MarketId(3)];
        assert!(validate_withdraw_route_no_duplicates(&route_ok));

        let route_dup = vec![MarketId(1), MarketId(2), MarketId(1)];
        assert!(!validate_withdraw_route_no_duplicates(&route_dup));
    }
}
