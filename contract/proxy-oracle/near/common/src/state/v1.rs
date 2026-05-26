use std::collections::{HashMap, HashSet};

use near_sdk::{collections::UnorderedMap, env, near, BorshStorageKey};
use templar_common::{
    contract::list,
    oracle::pyth::PriceIdentifier,
    versioned_state::{StateVersion, VersionedState},
    Nanoseconds,
};

use templar_proxy_oracle_kernel::{
    primitive::AccountId as KernelAccountId,
    proxy::{
        circuit_breaker::{
            AcceptedHistorySource, CircuitBreaker, CircuitBreakerError, CircuitBreakerOutcome,
            CircuitBreakerSet, CircuitBreakerSetConfig,
        },
        Proxy,
    },
};

use crate::{
    cache::{CachedProxyPrice, CachedProxyPriceStatus},
    input::Source,
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum StorageKey {
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
    fn edit_circuit_breaker_set<T>(&mut self, f: impl FnOnce(&mut CircuitBreakerSet) -> T) -> T {
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

    pub fn configure_circuit_breakers(
        &mut self,
        config: CircuitBreakerSetConfig,
    ) -> CircuitBreakerOutcome {
        self.edit_circuit_breaker_set(|set| set.set_config(config))
    }

    pub fn set_circuit_breaker_manual_trip(
        &mut self,
        is_manually_tripped: bool,
        actor: KernelAccountId,
        metadata: Option<Vec<u8>>,
    ) -> CircuitBreakerOutcome {
        self.edit_circuit_breaker_set(|set| {
            set.set_manual_trip(is_manually_tripped, actor, metadata)
        })
    }

    pub fn add_circuit_breaker(
        &mut self,
        breaker_id: u32,
        breaker: CircuitBreaker,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.edit_circuit_breaker_set(|set| {
            if set.breaker_count() >= crate::governance::MAX_CIRCUIT_BREAKERS_PER_PROXY {
                return Err(CircuitBreakerError::TooManyBreakers);
            }
            set.add(breaker_id, breaker)
        })
    }

    pub fn remove_circuit_breaker(
        &mut self,
        breaker_id: u32,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.edit_circuit_breaker_set(|set| set.remove(breaker_id))
    }

    pub fn set_enforced(
        &mut self,
        breaker_id: u32,
        is_enforced: bool,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.edit_circuit_breaker_set(|set| set.set_enforced(breaker_id, is_enforced))
    }

    pub fn rearm(
        &mut self,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.edit_circuit_breaker_set(|set| {
            set.rearm(breaker_id, armed_after_ns, accepted_history_source)
        })
    }
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct State {
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
        price_ids
            .into_iter()
            .filter(|price_id| self.proxy_exists(price_id))
            .map(|price_id| (price_id, self.cached_prices.get(&price_id)))
            .collect()
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
        if let Some(proxy) = proxy {
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
        } else {
            let proxy_removed = self.proxies.remove(&id).is_some();
            let breaker_set_removed = self.circuit_breakers.remove(&id).is_some();
            let cached_price_removed = self.cached_prices.remove(&id).is_some();
            let changed = proxy_removed || breaker_set_removed || cached_price_removed;
            if changed {
                self.bump_cache_epoch(id);
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
        let PendingProxyPriceUpdate {
            price_id,
            proxy,
            epoch,
        } = pending;

        if self.cache_epoch(price_id) != epoch || !self.proxy_exists(&price_id) {
            return None;
        }

        let Some(mut set) = self.circuit_breakers.get(&price_id) else {
            env::panic_str(&format!(
                "Circuit breaker set not found for price {price_id}"
            ));
        };
        let status = f(&proxy, &mut set);
        self.circuit_breakers.insert(&price_id, &set);
        self.cached_prices.insert(
            &price_id,
            &CachedProxyPrice {
                updated_at_ns: now,
                status: status.clone(),
            },
        );
        Some(status)
    }
}

impl StateVersion for State {
    const VERSION: u32 = 1;

    type NewArgs = ();

    fn new((): Self::NewArgs) -> VersionedState<Self> {
        VersionedState::new(Self {
            proxies: UnorderedMap::new(StorageKey::Proxies),
            circuit_breakers: UnorderedMap::new(StorageKey::CircuitBreakers),
            cached_prices: UnorderedMap::new(StorageKey::CachedPrices),
            cache_epochs: UnorderedMap::new(StorageKey::CacheEpochs),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use near_sdk::{test_utils::VMContextBuilder, testing_env};
    use templar_common::Decimal;
    use templar_proxy_oracle_kernel::proxy::{
        aggregator::Aggregator, circuit_breaker::StepwiseChange, FreshnessFilter,
    };

    use crate::{governance::MAX_CIRCUIT_BREAKERS_PER_PROXY, request::OracleRequest};

    fn state() -> State {
        State {
            proxies: UnorderedMap::new(StorageKey::Proxies),
            circuit_breakers: UnorderedMap::new(StorageKey::CircuitBreakers),
            cached_prices: UnorderedMap::new(StorageKey::CachedPrices),
            cache_epochs: UnorderedMap::new(StorageKey::CacheEpochs),
        }
    }

    fn proxy(price_id: PriceIdentifier) -> Proxy<Source> {
        Proxy::new(
            Aggregator::median_low([OracleRequest::pyth(
                "pyth-oracle.near".parse().unwrap(),
                price_id,
            )
            .into()]),
            FreshnessFilter::empty(),
        )
    }

    fn breaker() -> CircuitBreaker {
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: Decimal::from_str("0.10").unwrap(),
        })
    }

    #[test]
    fn proxy_entry_add_circuit_breaker_enforces_maximum_count() {
        testing_env!(VMContextBuilder::new().build());
        let price_id = PriceIdentifier([0x11; 32]);
        let mut state = state();
        state.set_proxy(price_id, Some(proxy(price_id)));

        for breaker_id in 0..u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap() {
            state
                .proxy_entry_mut(price_id)
                .unwrap()
                .add_circuit_breaker(breaker_id, breaker())
                .unwrap();
        }

        assert_eq!(
            state
                .proxy_entry_mut(price_id)
                .unwrap()
                .add_circuit_breaker(
                    u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap(),
                    breaker(),
                ),
            Err(CircuitBreakerError::TooManyBreakers)
        );
    }
}
