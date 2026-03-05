//! Policy adapters around curator-primitives for NEAR vault types.
// Note: adapters that touch templar_common types live here (not in curator-primitives)
// to keep curator-primitives chain-agnostic and dependency-light.

use crate::convert::IntoTargetId;
use near_sdk::{env, near};
use std::ops::{Deref, DerefMut};
use templar_common::vault::{Event, MarketId};

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
#[derive(Clone, Default)]
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
