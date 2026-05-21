use std::collections::{HashMap, HashSet};

use near_sdk::{collections::UnorderedMap, env, near, BorshStorageKey};
use templar_common::{
    contract::list,
    governance::Governance,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
    Nanoseconds,
};

use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};

use crate::{
    cache::{CachedProxyPrice, CachedProxyPriceStatus},
    governance::Operation,
    input::Source,
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum StorageKey {
    Governance,
    Proxies,
    CircuitBreakers,
    CachedPrices,
    CacheEpochs,
}

#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingProxyPriceUpdate {
    pub price_id: PriceIdentifier,
    pub proxy: Proxy<Source>,
    pub epoch: u64,
}

pub struct ProxyEntry<'a> {
    state: &'a State,
    id: PriceIdentifier,
    proxy: Proxy<Source>,
}

pub struct ProxyEntryMut<'a> {
    state: &'a mut State,
    id: PriceIdentifier,
}

impl ProxyEntry<'_> {
    pub fn prepare_price_update(&self) -> PendingProxyPriceUpdate {
        PendingProxyPriceUpdate {
            price_id: self.id,
            proxy: self.proxy.clone(),
            epoch: self.state.cache_epoch(self.id),
        }
    }
}

impl ProxyEntryMut<'_> {
    pub fn edit_circuit_breaker_set<T>(
        &mut self,
        f: impl FnOnce(&mut CircuitBreakerSet) -> T,
    ) -> T {
        let original = self
            .state
            .circuit_breakers
            .get(&self.id)
            .unwrap_or_else(|| env::panic_str("Circuit breaker set not found"));
        let mut set = original.clone();
        let value = f(&mut set);
        if set != original {
            self.state.invalidate_price_cache(self.id);
            self.state.circuit_breakers.insert(&self.id, &set);
        }
        value
    }
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
    pub governance: Governance<Operation>,
    pub(crate) proxies: UnorderedMap<PriceIdentifier, Proxy<Source>>,
    pub(crate) circuit_breakers: UnorderedMap<PriceIdentifier, CircuitBreakerSet>,
    pub(crate) cached_prices: UnorderedMap<PriceIdentifier, CachedProxyPrice>,
    pub(crate) cache_epochs: UnorderedMap<PriceIdentifier, u64>,
}

impl State {
    pub fn list_proxies(&self, offset: Option<u32>, count: Option<u32>) -> Vec<PriceIdentifier> {
        list(self.proxies.keys(), offset, count)
    }

    pub fn get_proxy(&self, id: PriceIdentifier) -> Option<Proxy<Source>> {
        self.proxies.get(&id)
    }

    pub fn proxy_exists(&self, id: &PriceIdentifier) -> bool {
        self.proxies.get(id).is_some()
    }

    pub fn get_proxy_circuit_breaker_set(&self, id: PriceIdentifier) -> Option<CircuitBreakerSet> {
        self.circuit_breakers.get(&id)
    }

    pub fn get_cached_proxy_price(&self, id: PriceIdentifier) -> Option<CachedProxyPrice> {
        self.cached_prices.get(&id)
    }

    pub fn list_cached_proxy_prices(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<CachedProxyPrice>> {
        HashMap::from_iter(
            HashSet::<PriceIdentifier>::from_iter(price_ids)
                .into_iter()
                .filter(|price_id| self.proxy_exists(price_id))
                .map(|price_id| (price_id, self.cached_prices.get(&price_id))),
        )
    }

    pub fn cache_epoch(&self, id: PriceIdentifier) -> u64 {
        self.cache_epochs.get(&id).unwrap_or(0)
    }

    pub fn proxy_entry(&self, id: PriceIdentifier) -> Option<ProxyEntry<'_>> {
        let proxy = self.proxies.get(&id)?;
        if self.circuit_breakers.get(&id).is_none() {
            env::panic_str("Circuit breaker set not found");
        }
        Some(ProxyEntry {
            state: self,
            id,
            proxy,
        })
    }

    pub fn proxy_entry_mut(&mut self, id: PriceIdentifier) -> Option<ProxyEntryMut<'_>> {
        self.proxies.get(&id)?;
        if self.circuit_breakers.get(&id).is_none() {
            env::panic_str("Circuit breaker set not found");
        }
        Some(ProxyEntryMut { state: self, id })
    }

    fn bump_cache_epoch(&mut self, id: PriceIdentifier) {
        let next = self
            .cache_epoch(id)
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Cache epoch overflow"));
        self.cache_epochs.insert(&id, &next);
    }

    fn invalidate_price_cache(&mut self, id: PriceIdentifier) {
        self.cached_prices.remove(&id);
        self.bump_cache_epoch(id);
    }

    pub fn set_proxy(&mut self, id: PriceIdentifier, proxy: Option<Proxy<Source>>) {
        match proxy {
            Some(proxy) => {
                let proxy_changed = self.proxies.get(&id).as_ref() != Some(&proxy);
                let missing_breaker_set = self.circuit_breakers.get(&id).is_none();
                self.proxies.insert(&id, &proxy);
                if missing_breaker_set {
                    self.circuit_breakers
                        .insert(&id, &CircuitBreakerSet::empty());
                }
                if proxy_changed || missing_breaker_set {
                    self.invalidate_price_cache(id);
                }
            }
            None => {
                let changed = self.proxies.remove(&id).is_some()
                    || self.circuit_breakers.remove(&id).is_some()
                    || self.cached_prices.get(&id).is_some();
                if changed {
                    self.invalidate_price_cache(id);
                }
            }
        }
    }

    pub fn finish_price_update_if_current<F>(
        &mut self,
        pending: PendingProxyPriceUpdate,
        now: Nanoseconds,
        f: F,
    ) -> Option<CachedProxyPriceStatus>
    where
        F: FnOnce(&Proxy<Source>, &mut CircuitBreakerSet) -> CachedProxyPriceStatus,
    {
        if self.cache_epoch(pending.price_id) != pending.epoch
            || !self.proxy_exists(&pending.price_id)
        {
            return None;
        }

        let mut set = self
            .circuit_breakers
            .get(&pending.price_id)
            .unwrap_or_else(CircuitBreakerSet::empty);
        let status = f(&pending.proxy, &mut set);
        self.circuit_breakers.insert(&pending.price_id, &set);
        self.cached_prices.insert(
            &pending.price_id,
            &CachedProxyPrice {
                updated_at_ns: now,
                status: status.clone(),
            },
        );
        Some(status)
    }
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
            cache_epochs: UnorderedMap::new(StorageKey::CacheEpochs),
        })
    }
}
