use near_sdk::{collections::UnorderedMap, near, BorshStorageKey};
use templar_common::{
    governance::Governance,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
};

use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};

use crate::{cache::CachedProxyPrice, governance::Operation, input::Source};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum StorageKey {
    Governance,
    Proxies,
    CircuitBreakers,
    CachedPrices,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub governance: Governance<Operation>,
    pub proxies: UnorderedMap<PriceIdentifier, Proxy<Source>>,
    pub circuit_breakers: UnorderedMap<PriceIdentifier, CircuitBreakerSet>,
    pub cached_prices: UnorderedMap<PriceIdentifier, CachedProxyPrice>,
}

impl StateVersion for State {
    const VERSION: u32 = 2;

    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            governance: Governance::new(StorageKey::Governance),
            proxies: UnorderedMap::new(StorageKey::Proxies),
            circuit_breakers: UnorderedMap::new(StorageKey::CircuitBreakers),
            cached_prices: UnorderedMap::new(StorageKey::CachedPrices),
        })
    }
}
