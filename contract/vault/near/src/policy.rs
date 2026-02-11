//! Policy adapters around curator-primitives for NEAR vault types.
// Note: adapters that touch templar_common types live here (not in curator-primitives)
// to keep curator-primitives chain-agnostic and dependency-light.

use crate::convert::{IntoMarketId, IntoTargetId};
use near_sdk::{env, near};
use std::ops::{Deref, DerefMut};
use templar_common::vault::{CapGroupRecord as CommonCapGroupRecord, Event, MarketId};
use templar_curator_primitives::policy::target_set::{
    build_refresh_plan_from_targets, build_withdraw_plan_from_target_principals,
    find_duplicate_target_id, find_locked_targets as find_locked_target_ids,
    get_locked_targets as get_locked_target_ids, is_target_locked, validate_no_duplicate_targets,
};
use templar_curator_primitives::{
    available_capacity_from_fields, can_allocate_from_fields, cap_group_from_fields,
    cap_group_record_from_fields, effective_cap_from_fields, enforce_from_fields,
};

// Re-export curator-primitives types for external consumers
pub use templar_curator_primitives::policy::{
    cap_group::{
        CapGroup, CapGroupError, CapGroupId as PrimitiveCapGroupId,
        CapGroupRecord as PrimitiveCapGroupRecord,
    },
    market_lock::{MarketLock, MarketLockSet},
    refresh_plan::{RefreshPlan as PrimitiveRefreshPlan, RefreshPlanError},
    supply_queue::{SupplyQueue as PrimitiveSupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity,
        WithdrawRoute as PrimitiveWithdrawRoute, WithdrawRouteEntry, WithdrawRouteError,
    },
};

pub const ERR_MARKET_LOCKED: &str = "Market is locked";

/// NEAR wrapper for the curator supply queue (preserves Vec<MarketId> layout).
#[near(serializers = [borsh, serde])]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupplyQueue(pub Vec<MarketId>);

impl Deref for SupplyQueue {
    type Target = Vec<MarketId>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SupplyQueue {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<MarketId>> for SupplyQueue {
    fn from(markets: Vec<MarketId>) -> Self {
        Self(markets)
    }
}

impl From<SupplyQueue> for Vec<MarketId> {
    fn from(queue: SupplyQueue) -> Self {
        queue.0
    }
}

/// NEAR wrapper for the curator withdraw route (preserves Vec<MarketId> layout).
#[near(serializers = [borsh, serde])]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WithdrawRoute(pub Vec<MarketId>);

impl Deref for WithdrawRoute {
    type Target = Vec<MarketId>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for WithdrawRoute {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<MarketId>> for WithdrawRoute {
    fn from(markets: Vec<MarketId>) -> Self {
        Self(markets)
    }
}

impl From<WithdrawRoute> for Vec<MarketId> {
    fn from(route: WithdrawRoute) -> Self {
        route.0
    }
}

/// NEAR wrapper for market execution locks (backed by curator MarketLockSet).
#[near(serializers = [borsh, serde])]
#[derive(Clone, Debug, Default)]
pub struct MarketExecutionLock {
    inner: MarketLockSet,
}

impl MarketExecutionLock {
    fn acquire_lock_or_panic(set: &MarketLockSet, market: MarketId, now_ns: u64) -> MarketLockSet {
        let lock = MarketLock::new(market.into_target_id(), now_ns);
        set.acquire(lock, now_ns)
            .unwrap_or_else(|_| env::panic_str(ERR_MARKET_LOCKED))
    }

    pub fn lock(&mut self, market: MarketId) {
        let now = env::block_timestamp();
        let new_set = Self::acquire_lock_or_panic(&self.inner, market, now);

        Event::LockChange {
            is_locked: true,
            market,
        }
        .emit();

        self.inner = new_set;
    }

    pub fn unlock(&mut self, market: MarketId) {
        Event::LockChange {
            is_locked: false,
            market,
        }
        .emit();
        self.inner = self.inner.release(market.into_target_id());
    }

    pub fn clear(&mut self) {
        self.inner = self.inner.clear();
    }

    pub fn is_locked(&self, market: MarketId) -> bool {
        self.inner
            .is_locked(market.into_target_id(), env::block_timestamp())
    }

    pub fn is_locked_all(&self) -> bool {
        self.inner.active_count(env::block_timestamp()) > 0
    }

    pub fn from_markets(markets: Vec<MarketId>, locked_at_ns: u64) -> Self {
        let mut set = MarketLockSet::new();
        for market in markets {
            set = Self::acquire_lock_or_panic(&set, market, locked_at_ns);
        }
        Self { inner: set }
    }

    #[must_use]
    pub fn inner(&self) -> &MarketLockSet {
        &self.inner
    }
}

/// Convert a common CapGroupRecord to a curator-primitives CapGroup for use with pure functions.
///
/// The common module stores cap and relative_cap directly on the record,
/// while curator-primitives separates them into a CapGroup struct.
pub fn to_primitive_cap_group(record: &CommonCapGroupRecord) -> CapGroup {
    cap_group_from_fields(record.cap.0, record.relative_cap)
}

/// Convert a common CapGroupRecord to a curator-primitives CapGroupRecord.
pub fn to_primitive_cap_group_record(record: &CommonCapGroupRecord) -> PrimitiveCapGroupRecord {
    cap_group_record_from_fields(record.cap.0, record.relative_cap, record.principal)
}

/// Check if an allocation is allowed for a cap group using common types.
///
/// This is a convenience wrapper around the curator-primitives `CapGroup::can_allocate` method.
pub fn can_allocate_to_common_cap_group(
    record: &CommonCapGroupRecord,
    amount: u128,
    total_assets: u128,
) -> bool {
    can_allocate_from_fields(
        record.cap.0,
        record.relative_cap,
        record.principal,
        amount,
        total_assets,
    )
}

/// Enforce cap group constraints using common types.
///
/// This is a convenience wrapper around the curator-primitives `CapGroup::enforce` method.
pub fn enforce_common_cap_group(
    record: &CommonCapGroupRecord,
    amount: u128,
    total_assets: u128,
) -> Result<(), CapGroupError> {
    enforce_from_fields(
        record.cap.0,
        record.relative_cap,
        record.principal,
        amount,
        total_assets,
    )
}

/// Compute the effective cap for a common CapGroupRecord.
pub fn compute_effective_cap_for_common(record: &CommonCapGroupRecord, total_assets: u128) -> u128 {
    effective_cap_from_fields(record.cap.0, record.relative_cap, total_assets)
}

/// Compute available capacity for a common CapGroupRecord.
pub fn compute_available_capacity_for_common(
    record: &CommonCapGroupRecord,
    total_assets: u128,
) -> u128 {
    available_capacity_from_fields(
        record.cap.0,
        record.relative_cap,
        record.principal,
        total_assets,
    )
}

/// Validate a supply queue represented as a Vec<MarketId>.
///
/// The NEAR vault uses a simple Vec<MarketId> for its supply queue,
/// while curator-primitives uses a more detailed SupplyQueueEntry structure.
/// This function validates the basic requirements: no duplicates.
pub fn validate_supply_queue_no_duplicates(queue: &[MarketId]) -> bool {
    let target_ids: Vec<u32> = queue.iter().map(IntoTargetId::into_target_id).collect();
    validate_no_duplicate_targets(&target_ids)
}

/// Returns the first duplicate market ID in insertion order.
#[must_use]
pub fn find_duplicate_market_id(markets: &[MarketId]) -> Option<MarketId> {
    let target_ids: Vec<u32> = markets.iter().map(IntoTargetId::into_target_id).collect();
    find_duplicate_target_id(&target_ids).map(IntoMarketId::into_market_id)
}

/// Build a withdraw route from market principals.
///
/// Converts NEAR MarketId to TargetId for use with curator-primitives,
/// then returns the validated route.
pub fn build_withdraw_route_from_markets(
    principals: &[(MarketId, u128)],
    target_amount: u128,
) -> Result<Vec<(MarketId, u128)>, WithdrawRouteError> {
    let target_principals: Vec<(u32, u128)> = principals
        .iter()
        .map(|(m, p)| (m.into_target_id(), *p))
        .collect();

    Ok(
        build_withdraw_plan_from_target_principals(&target_principals, target_amount)?
            .iter()
            .map(|(target_id, amount)| (target_id.into_market_id(), *amount))
            .collect(),
    )
}

/// Validate a withdraw route represented as Vec<MarketId>.
///
/// Checks for duplicates in the route.
pub fn validate_withdraw_route_no_duplicates(route: &[MarketId]) -> bool {
    let target_ids: Vec<u32> = route.iter().map(IntoTargetId::into_target_id).collect();
    validate_no_duplicate_targets(&target_ids)
}

/// Check if any markets in the list are locked.
///
/// Converts MarketId to TargetId for use with curator-primitives.
pub fn find_locked_markets(
    lock_set: &MarketLockSet,
    markets: &[MarketId],
    current_ns: u64,
) -> Vec<MarketId> {
    let targets: Vec<u32> = markets.iter().map(|m| m.into_target_id()).collect();
    let locked = find_locked_target_ids(lock_set, &targets, current_ns);
    locked
        .into_iter()
        .map(IntoMarketId::into_market_id)
        .collect()
}

/// Check if a specific market is locked.
pub fn is_market_id_locked(lock_set: &MarketLockSet, market: MarketId, current_ns: u64) -> bool {
    is_target_locked(lock_set, market.into_target_id(), current_ns)
}

/// Get all locked market IDs.
pub fn get_locked_market_ids(lock_set: &MarketLockSet, current_ns: u64) -> Vec<MarketId> {
    get_locked_target_ids(lock_set, current_ns)
        .into_iter()
        .map(IntoMarketId::into_market_id)
        .collect()
}

/// Build a curator refresh plan from NEAR market IDs and cooldown state.
pub fn build_refresh_plan_from_market_ids(
    markets: &[MarketId],
    cooldown_ns: u64,
    last_refresh_ns: u64,
) -> Result<PrimitiveRefreshPlan, RefreshPlanError> {
    let targets = markets
        .iter()
        .map(IntoTargetId::into_target_id)
        .collect::<Vec<_>>();
    build_refresh_plan_from_targets(&targets, cooldown_ns, last_refresh_ns)
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
        assert_eq!(cap.absolute_cap.map(|c| c.get()), Some(1000));
        assert_eq!(cap.relative_cap, Some(Wad::from(WAD / 2)));
    }

    #[test]
    fn cap_group_bridge_uses_shared_field_adapters() {
        let record = CommonCapGroupRecord {
            cap: U128(1000),
            relative_cap: Wad::one(),
            principal: 300,
        };

        assert!(can_allocate_to_common_cap_group(&record, 500, 2000));
        assert!(!can_allocate_to_common_cap_group(&record, 800, 2000));
        assert!(enforce_common_cap_group(&record, 500, 2000).is_ok());
        assert!(matches!(
            enforce_common_cap_group(&record, 800, 2000),
            Err(CapGroupError::ExceedsAbsoluteCap { .. })
        ));
        assert_eq!(compute_effective_cap_for_common(&record, 500), 500);
        assert_eq!(compute_available_capacity_for_common(&record, 500), 200);
    }

    #[test]
    fn duplicate_checks_bridge_market_ids() {
        let queue_ok = vec![MarketId(1), MarketId(2), MarketId(3)];
        assert!(validate_supply_queue_no_duplicates(&queue_ok));
        assert!(validate_withdraw_route_no_duplicates(&queue_ok));
        assert_eq!(find_duplicate_market_id(&queue_ok), None);

        let queue_dup = vec![MarketId(1), MarketId(2), MarketId(1)];
        assert!(!validate_supply_queue_no_duplicates(&queue_dup));
        assert!(!validate_withdraw_route_no_duplicates(&queue_dup));
        assert_eq!(find_duplicate_market_id(&queue_dup), Some(MarketId(1)));
    }

    #[test]
    fn lock_helpers_bridge_market_ids() {
        let lock_set = MarketLockSet::new()
            .acquire(MarketLock::new(2, 1_000), 1_000)
            .unwrap();
        let markets = vec![MarketId(1), MarketId(2), MarketId(3)];

        assert_eq!(
            find_locked_markets(&lock_set, &markets, 1_500),
            vec![MarketId(2)]
        );
        assert!(is_market_id_locked(&lock_set, MarketId(2), 1_500));
        assert_eq!(get_locked_market_ids(&lock_set, 1_500), vec![MarketId(2)]);
    }

    #[test]
    fn withdraw_and_refresh_plan_builders_bridge_market_ids() {
        let principals = vec![(MarketId(1), 100), (MarketId(2), 200), (MarketId(3), 300)];
        let route = build_withdraw_route_from_markets(&principals, 250).unwrap();
        assert_eq!(
            route,
            vec![(MarketId(3), 300), (MarketId(2), 200), (MarketId(1), 100)]
        );

        let refresh =
            build_refresh_plan_from_market_ids(&[MarketId(3), MarketId(1)], 100, 50).unwrap();
        assert_eq!(refresh.targets, vec![3, 1]);
        assert_eq!(refresh.cooldown_ns(), 100);
        assert_eq!(refresh.last_refresh_ns(), Some(50));
    }
}
