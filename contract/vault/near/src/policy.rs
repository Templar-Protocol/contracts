//! Policy adapters around curator-primitives for NEAR vault types.
// Note: adapters that touch templar_common types live here (not in curator-primitives)
// to keep curator-primitives chain-agnostic and dependency-light.

use crate::convert::IntoTargetId;
use near_sdk::{env, near};
use std::ops::{Deref, DerefMut};
use templar_common::vault::{Event, MarketId};
use templar_vault_kernel::TimestampNs;

pub use templar_curator_primitives::policy::{
    cap_group::{
        CapGroup, CapGroupError, CapGroupId as PrimitiveCapGroupId,
        CapGroupRecord as PrimitiveCapGroupRecord,
    },
    market_lock::{FencingToken, LeaseDurationNs, LeaseOwner, MarketLease, MarketLeaseRegistry},
    refresh_plan::{
        RefreshPlan as PrimitiveRefreshPlan, RefreshPlanError, RefreshTargetStatus, RefreshThrottle,
    },
    supply_queue::{SupplyQueue as PrimitiveSupplyQueue, SupplyQueueEntry, SupplyQueueError},
    withdraw_route::{
        build_withdraw_route, build_withdraw_route_with_liquidity,
        WithdrawRoute as PrimitiveWithdrawRoute, WithdrawRouteEntry, WithdrawRouteError,
    },
};

pub const ERR_MARKET_LEASED: &str = "Market is leased";

macro_rules! define_market_id_vec_wrapper {
    ($name:ident) => {
        #[near(serializers = [borsh, serde])]
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name(pub Vec<MarketId>);

        impl Deref for $name {
            type Target = Vec<MarketId>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl From<Vec<MarketId>> for $name {
            fn from(markets: Vec<MarketId>) -> Self {
                Self(markets)
            }
        }

        impl From<$name> for Vec<MarketId> {
            fn from(markets: $name) -> Self {
                markets.0
            }
        }
    };
}

// NEAR wrapper for the curator supply queue (preserves Vec<MarketId> layout).
define_market_id_vec_wrapper!(SupplyQueue);

// NEAR wrapper for the curator withdraw route (preserves Vec<MarketId> layout).
define_market_id_vec_wrapper!(WithdrawRoute);

/// NEAR wrapper for market execution leases backed by curator MarketLeaseRegistry.
#[near(serializers = [borsh, serde])]
#[derive(Clone, Default)]
pub struct MarketExecutionLock {
    inner: MarketLeaseRegistry,
}

impl MarketExecutionLock {
    fn lease_owner(op_id: u64) -> LeaseOwner {
        LeaseOwner(op_id)
    }

    fn acquire_lease_or_panic(
        registry: &MarketLeaseRegistry,
        market: MarketId,
        op_id: u64,
        ttl_ns: u64,
        now_ns: u64,
    ) -> (MarketLeaseRegistry, MarketLease) {
        registry
            .try_acquire(
                market.into_target_id(),
                Self::lease_owner(op_id),
                Some(op_id),
                TimestampNs(now_ns),
                LeaseDurationNs(ttl_ns),
            )
            .unwrap_or_else(|_| env::panic_str(ERR_MARKET_LEASED))
    }

    pub fn lock(&mut self, market: MarketId, op_id: u64, ttl_ns: u64) -> MarketLease {
        let now = env::block_timestamp();
        let (next_registry, lease) =
            Self::acquire_lease_or_panic(&self.inner, market, op_id, ttl_ns, now);

        Event::LockChange {
            is_locked: true,
            market,
        }
        .emit();

        self.inner = next_registry;
        lease
    }

    pub fn unlock(&mut self, market: MarketId, op_id: u64, token: FencingToken) {
        self.inner = self
            .inner
            .release_if_owned_with_token(market.into_target_id(), &Self::lease_owner(op_id), token)
            .unwrap_or_else(|_| env::panic_str(ERR_MARKET_LEASED));

        Event::LockChange {
            is_locked: false,
            market,
        }
        .emit();
    }

    pub fn clear(&mut self) {
        self.inner = self.inner.clear();
    }

    pub fn is_locked(&self, market: MarketId) -> bool {
        self.inner
            .is_leased(market.into_target_id(), TimestampNs(env::block_timestamp()))
    }

    pub fn assert_current(&self, market: MarketId, lease: &MarketLease) {
        self.assert_current_token(market, lease.fencing_token);
    }

    pub fn assert_current_token(&self, market: MarketId, token: FencingToken) {
        self.inner
            .assert_token_current(
                market.into_target_id(),
                token,
                TimestampNs(env::block_timestamp()),
            )
            .unwrap_or_else(|_| env::panic_str(ERR_MARKET_LEASED));
    }

    #[must_use]
    pub fn is_current_token(&self, market: MarketId, token: FencingToken) -> bool {
        self.inner
            .assert_token_current(
                market.into_target_id(),
                token,
                TimestampNs(env::block_timestamp()),
            )
            .is_ok()
    }

    #[must_use]
    pub fn has_active_lease(&self, market: MarketId) -> bool {
        self.inner
            .is_leased(market.into_target_id(), TimestampNs(env::block_timestamp()))
    }

    pub fn from_markets(markets: Vec<MarketId>, locked_at_ns: u64) -> Self {
        let mut registry = MarketLeaseRegistry::default();
        for (index, market) in markets.into_iter().enumerate() {
            let op_id = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            let (next_registry, _) = Self::acquire_lease_or_panic(
                &registry,
                market,
                op_id,
                u64::MAX.saturating_sub(locked_at_ns),
                locked_at_ns,
            );
            registry = next_registry;
        }
        Self { inner: registry }
    }

    #[must_use]
    pub fn inner(&self) -> &MarketLeaseRegistry {
        &self.inner
    }
}
