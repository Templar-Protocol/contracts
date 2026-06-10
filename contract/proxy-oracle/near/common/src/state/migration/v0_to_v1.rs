use near_sdk::near;
use templar_common::{oracle::pyth::PriceIdentifier, versioned_state::StateTransformer};
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};

use crate::{
    input::Source,
    state::{self, legacy::v0, v1},
};

impl From<v0::ProxyPriceTransformer> for crate::input::ProxyPriceTransformer {
    fn from(value: v0::ProxyPriceTransformer) -> Self {
        Self {
            request: value.request,
            call: value.call,
            action: value.action,
        }
    }
}

impl From<v0::LegacySource> for Source {
    fn from(value: v0::LegacySource) -> Self {
        match value {
            v0::LegacySource::Request(request) => Self::Request(request),
            v0::LegacySource::Transformer(transformer) => Self::Transformer(transformer.into()),
        }
    }
}

impl From<v0::Entry> for templar_proxy_oracle_kernel::proxy::WeightedSource<Source> {
    fn from(value: v0::Entry) -> Self {
        Self::new(Source::from(value.source), value.weight)
    }
}

impl From<v0::Filter> for templar_proxy_oracle_kernel::proxy::FreshnessFilter {
    fn from(value: v0::Filter) -> Self {
        Self::new(value.max_age, value.max_clock_drift)
    }
}

impl From<v0::Proxy> for Proxy<Source> {
    fn from(value: v0::Proxy) -> Self {
        use templar_proxy_oracle_kernel::proxy::aggregator::{
            method::{median::MedianLow, priority::Priority},
            Aggregator,
        };

        let freshness_filter = templar_proxy_oracle_kernel::proxy::FreshnessFilter::from(
            value.aggregator.filter.clone(),
        );
        let source_count = u32::try_from(value.entries.len())
            .unwrap_or(u32::MAX)
            .max(1);

        let aggregator = match value.aggregator.method {
            v0::AggregationMethod::MedianLow => {
                let mut aggregator = MedianLow::new(
                    value
                        .entries
                        .into_iter()
                        .map(templar_proxy_oracle_kernel::proxy::WeightedSource::from),
                );
                aggregator.min_sources = value
                    .aggregator
                    .filter
                    .min_sources
                    .unwrap_or(1)
                    .clamp(1, source_count);
                Aggregator::MedianLow(aggregator)
            }
            v0::AggregationMethod::Priority => {
                let mut sources = value.entries.into_iter().enumerate().collect::<Vec<_>>();
                sources.sort_by(|(left_index, left), (right_index, right)| {
                    right
                        .weight
                        .cmp(&left.weight)
                        .then(left_index.cmp(right_index))
                });
                let ordered_sources = sources
                    .into_iter()
                    .map(|(_, entry)| Source::from(entry.source));

                Aggregator::Priority(Priority::new(ordered_sources))
            }
        };

        Proxy::new(aggregator, freshness_filter)
    }
}

fn snapshot_proxies(
    proxies: &near_sdk::collections::UnorderedMap<PriceIdentifier, v0::Proxy>,
) -> Vec<(PriceIdentifier, Proxy<Source>)> {
    proxies
        .iter()
        .map(|(price_id, proxy)| (price_id, Proxy::from(proxy)))
        .collect()
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct V0ToV1;

impl StateTransformer for V0ToV1 {
    type Input = v0::State;
    type Output = v1::State;
    type Error = ();

    fn transform(&self, mut input: Self::Input) -> Result<Self::Output, Self::Error> {
        let proxies_snapshot = snapshot_proxies(&input.proxies);

        input.governance.proposals.clear();
        input.governance.proposals.flush();
        input.proxies.clear();
        drop(input);

        let mut proxies = near_sdk::collections::UnorderedMap::new(v1::StorageKey::Proxies);
        let mut circuit_breakers =
            near_sdk::collections::UnorderedMap::new(v1::StorageKey::CircuitBreakers);
        let cached_prices = near_sdk::collections::UnorderedMap::new(v1::StorageKey::CachedPrices);
        let cache_epochs = near_sdk::collections::UnorderedMap::new(v1::StorageKey::CacheEpochs);
        for (price_id, proxy) in proxies_snapshot {
            proxies.insert(&price_id, &proxy);
            circuit_breakers.insert(&price_id, &CircuitBreakerSet::empty());
        }

        Ok(state::v1::State {
            proxies,
            circuit_breakers,
            cached_prices,
            cache_epochs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use near_sdk::{test_utils::VMContextBuilder, testing_env, AccountId};
    use templar_common::{
        oracle::pyth::PriceIdentifier,
        versioned_state::{read_state_version, write_state_version, StateTransformer},
        Nanoseconds,
    };

    use crate::{
        request::OracleRequest,
        state::{legacy::v0, migration::v0_to_v1::V0ToV1},
    };

    fn context() {
        testing_env!(VMContextBuilder::new().build());
    }

    fn account(id: &str) -> AccountId {
        id.parse().unwrap()
    }

    #[test]
    fn v0_patch_deserializes_with_local_schema() {
        context();

        let patch: HashMap<Vec<u8>, Vec<u8>> = near_sdk::borsh::from_slice(include_bytes!(
            "../../../../contract/tests/migration/v0_state_patch.borsh"
        ))
        .unwrap();

        for (key, value) in patch {
            near_sdk::env::storage_write(&key, &value);
        }

        let state = near_sdk::env::state_read::<v0::State>().unwrap();
        assert_eq!(state.governance.next_id, 6);
        assert_eq!(state.proxies.len(), 3);
        assert_eq!(state.governance.proposals.iter().count(), 2);
        assert_eq!(state.proxies.iter().count(), 3);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn v0_to_v1_migrates_proxies_and_initializes_breaker_sets() {
        context();

        let btc = PriceIdentifier([0x11; 32]);
        let eth = PriceIdentifier([0x22; 32]);

        let mut old = v0::State {
            governance: v0::Governance {
                next_id: 9,
                ttl: Nanoseconds::from_secs(55),
                proposals: near_sdk::store::IterableMap::with_hasher(v0::StorageKey::Governance),
            },
            proxies: near_sdk::collections::UnorderedMap::new(v0::StorageKey::Proxies),
        };

        let median_proxy = v0::Proxy {
            aggregator: v0::Aggregator::median_low(v0::Filter {
                max_age: Some(Nanoseconds::from_secs(60)),
                max_clock_drift: Some(Nanoseconds::from_secs(10)),
                min_sources: Some(2),
            }),
            entries: vec![
                v0::Entry::new(OracleRequest::pyth(account("pyth.near"), btc), 3),
                v0::Entry::new(OracleRequest::redstone(account("redstone.near"), "BTC"), 1),
            ],
        };
        let priority_proxy = v0::Proxy {
            aggregator: v0::Aggregator::priority(v0::Filter {
                max_age: Some(Nanoseconds::from_secs(70)),
                max_clock_drift: Some(Nanoseconds::from_secs(20)),
                min_sources: Some(99),
            }),
            entries: vec![
                v0::Entry::new(OracleRequest::redstone(account("redstone.near"), "ETH"), 7),
                v0::Entry::new(OracleRequest::pyth(account("pyth.near"), eth), 7),
                v0::Entry::new(OracleRequest::pyth(account("pyth2.near"), eth), 3),
            ],
        };

        old.proxies.insert(&btc, &median_proxy);
        old.proxies.insert(&eth, &priority_proxy);
        old.governance.proposals.insert(
            7,
            v0::Proposal {
                operation: v0::Operation::SetProxy {
                    id: eth,
                    proxy: Some(priority_proxy.clone()),
                },
                created_at: Nanoseconds::from_secs(10),
                ttl: Nanoseconds::from_secs(15),
                created_by: account("owner.near"),
            },
        );
        old.governance.proposals.flush();

        near_sdk::env::state_write(&old);
        write_state_version(0);

        let new = V0ToV1.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 1);

        let migrated_btc = new.proxies.get(&btc).unwrap();
        assert_eq!(
            migrated_btc.freshness_filter,
            templar_proxy_oracle_kernel::proxy::FreshnessFilter::new(
                Some(Nanoseconds::from_secs(60)),
                Some(Nanoseconds::from_secs(10)),
            ),
        );
        match &migrated_btc.aggregator {
            templar_proxy_oracle_kernel::proxy::aggregator::Aggregator::MedianLow(aggregator) => {
                assert_eq!(aggregator.min_sources, 2);
                assert_eq!(aggregator.sources[0].weight, 3);
                assert_eq!(aggregator.sources[1].weight, 1);
            }
            other => panic!("unexpected aggregator: {other:?}"),
        }

        assert_eq!(
            new.circuit_breakers.get(&btc),
            Some(CircuitBreakerSet::empty())
        );
        assert_eq!(
            new.circuit_breakers.get(&eth),
            Some(CircuitBreakerSet::empty())
        );
        assert!(new.cached_prices.is_empty());
    }

    #[test]
    fn v0_to_v1_ignores_priority_min_sources_over_one() {
        context();

        let proxy = Proxy::from(v0::Proxy {
            aggregator: v0::Aggregator::priority(v0::Filter {
                max_age: None,
                max_clock_drift: None,
                min_sources: Some(2),
            }),
            entries: vec![v0::Entry::new(
                OracleRequest::redstone(account("redstone.near"), "BTC"),
                1,
            )],
        });

        match proxy.aggregator {
            templar_proxy_oracle_kernel::proxy::aggregator::Aggregator::Priority(priority) => {
                assert_eq!(priority.sources.len(), 1);
                assert!(matches!(
                    priority.sources[0],
                    Source::Request(OracleRequest::RedStone(_))
                ));
            }
            other => panic!("unexpected aggregator: {other:?}"),
        }
    }

    #[test]
    fn v0_to_v1_clamps_median_low_min_sources_to_available_entries() {
        context();

        let proxy = Proxy::from(v0::Proxy {
            aggregator: v0::Aggregator::median_low(v0::Filter {
                max_age: None,
                max_clock_drift: None,
                min_sources: Some(99),
            }),
            entries: vec![
                v0::Entry::new(OracleRequest::redstone(account("redstone.near"), "BTC"), 3),
                v0::Entry::new(
                    OracleRequest::pyth(account("pyth.near"), PriceIdentifier([1; 32])),
                    1,
                ),
            ],
        });

        match proxy.aggregator {
            templar_proxy_oracle_kernel::proxy::aggregator::Aggregator::MedianLow(aggregator) => {
                assert_eq!(aggregator.sources.len(), 2);
                assert_eq!(aggregator.min_sources, 2);
            }
            other => panic!("unexpected aggregator: {other:?}"),
        }
    }

    #[test]
    fn v0_to_v1_handles_empty_median_low_entries_without_panicking() {
        context();

        let proxy = Proxy::from(v0::Proxy {
            aggregator: v0::Aggregator::median_low(v0::Filter {
                max_age: None,
                max_clock_drift: None,
                min_sources: Some(99),
            }),
            entries: vec![],
        });

        match proxy.aggregator {
            templar_proxy_oracle_kernel::proxy::aggregator::Aggregator::MedianLow(aggregator) => {
                assert_eq!(aggregator.sources.len(), 0);
                assert_eq!(aggregator.min_sources, 1);
            }
            other => panic!("unexpected aggregator: {other:?}"),
        }
    }
}
