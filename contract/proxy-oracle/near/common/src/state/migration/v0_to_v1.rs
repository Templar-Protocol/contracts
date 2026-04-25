use near_sdk::near;
use templar_common::{
    governance::{Governance, Proposal},
    oracle::pyth::PriceIdentifier,
    versioned_state::StateTransformer,
};
use templar_proxy_oracle_kernel::proxy::{
    aggregator::method::{median::MedianLow, priority::Priority},
    Aggregator, FreshnessFilter, Proxy, WeightedSource,
};

use crate::{
    governance,
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

impl From<v0::Entry> for WeightedSource<Source> {
    fn from(value: v0::Entry) -> Self {
        Self::new(Source::from(value.source), value.weight)
    }
}

impl From<v0::Filter> for FreshnessFilter {
    fn from(value: v0::Filter) -> Self {
        Self::new(value.max_age, value.max_clock_drift)
    }
}

impl From<v0::Proxy> for Proxy<Source> {
    fn from(value: v0::Proxy) -> Self {
        let freshness_filter = FreshnessFilter::from(value.aggregator.filter.clone());

        let aggregator = match value.aggregator.method {
            v0::AggregationMethod::MedianLow => {
                let mut aggregator =
                    MedianLow::new(value.entries.into_iter().map(WeightedSource::from));
                aggregator.min_sources = value.aggregator.filter.min_sources.unwrap_or(1).max(1);
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

impl From<v0::Operation> for governance::Operation {
    fn from(value: v0::Operation) -> Self {
        match value {
            v0::Operation::SetProxy { id, proxy } => Self::SetProxy {
                id,
                proxy: proxy.map(Proxy::from),
            },
            v0::Operation::SetActionTtl { new_ttl } => Self::SetActionTtl { new_ttl },
        }
    }
}

fn migrate_proposal(proposal: Proposal<v0::Operation>) -> Proposal<governance::Operation> {
    Proposal {
        operation: proposal.operation.into(),
        created_at: proposal.created_at,
        ttl: proposal.ttl,
        created_by: proposal.created_by,
    }
}

fn snapshot_proposals(
    governance: &Governance<v0::Operation>,
) -> Vec<(u32, Proposal<governance::Operation>)> {
    (0..governance.next_id)
        .filter_map(|proposal_id| {
            governance
                .proposals
                .get(&proposal_id)
                .cloned()
                .map(|proposal| (proposal_id, migrate_proposal(proposal)))
        })
        .collect()
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
        let proposals = snapshot_proposals(&input.governance);
        let proxies_snapshot = snapshot_proxies(&input.proxies);
        let next_id = input.governance.next_id;
        let ttl = input.governance.ttl;

        input.governance.proposals.clear();
        input.governance.proposals.flush();
        input.proxies.clear();
        drop(input);

        let mut governance = Governance::new(v1::StorageKey::Governance);
        governance.next_id = next_id;
        governance.ttl = ttl;

        for (proposal_id, proposal) in proposals {
            governance.proposals.insert(proposal_id, proposal);
        }

        let mut proxies = near_sdk::collections::UnorderedMap::new(v1::StorageKey::Proxies);
        for (price_id, proxy) in proxies_snapshot {
            proxies.insert(&price_id, &proxy);
        }

        Ok(state::v1::State {
            governance,
            proxies,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use near_sdk::{test_utils::VMContextBuilder, testing_env, AccountId};
    use templar_common::{
        governance::{Governance, Proposal},
        oracle::pyth::PriceIdentifier,
        Nanoseconds,
        versioned_state::{read_state_version, write_state_version, StateTransformer},
    };

    use crate::{
        governance::Operation,
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
    fn v0_to_v1_migrates_proxy_map_and_governance() {
        context();

        let btc = PriceIdentifier([0x11; 32]);
        let eth = PriceIdentifier([0x22; 32]);
        let governance_id = 7;
        let created_by = account("owner.near");

        let mut old = v0::State {
            governance: Governance::new(v0::StorageKey::Governance),
            proxies: near_sdk::collections::UnorderedMap::new(v0::StorageKey::Proxies),
        };
        old.governance.next_id = 9;
        old.governance.ttl = Nanoseconds::from_secs(55);

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
        old.governance.proposals.insert(
            governance_id,
            Proposal {
                operation: v0::Operation::SetProxy {
                    id: eth,
                    proxy: Some(priority_proxy.clone()),
                },
                created_at: Nanoseconds::from_secs(10),
                ttl: Nanoseconds::from_secs(15),
                created_by: created_by.clone(),
            },
        );
        old.governance.proposals.flush();

        near_sdk::env::state_write(&old);
        write_state_version(0);

        let new = V0ToV1.run().unwrap();

        assert_eq!(read_state_version().unwrap(), 1);
        assert_eq!(new.governance.next_id, 9);
        assert_eq!(new.governance.ttl, Nanoseconds::from_secs(55));

        let migrated_btc = new.proxies.get(&btc).unwrap();
        assert_eq!(
            migrated_btc.freshness_filter,
            FreshnessFilter::new(
                Some(Nanoseconds::from_secs(60)),
                Some(Nanoseconds::from_secs(10)),
            ),
        );
        match &migrated_btc.aggregator {
            Aggregator::MedianLow(aggregator) => {
                assert_eq!(aggregator.min_sources, 2);
                assert_eq!(aggregator.sources[0].weight, 3);
                assert_eq!(aggregator.sources[1].weight, 1);
            }
            other => panic!("unexpected aggregator: {other:?}"),
        }

        let proposal = new.governance.proposals.get(&governance_id).unwrap();
        assert_eq!(proposal.created_at, Nanoseconds::from_secs(10));
        assert_eq!(proposal.ttl, Nanoseconds::from_secs(15));
        assert_eq!(proposal.created_by, created_by);

        match &proposal.operation {
            Operation::SetProxy {
                id,
                proxy: Some(proxy),
            } => {
                assert_eq!(id, &eth);
                assert_eq!(
                    proxy.freshness_filter,
                    FreshnessFilter::new(
                        Some(templar_common::Nanoseconds::from_secs(70)),
                        Some(templar_common::Nanoseconds::from_secs(20)),
                    ),
                );
                match &proxy.aggregator {
                    Aggregator::Priority(priority) => {
                        assert_eq!(priority.sources.len(), 3);
                        assert!(matches!(
                            priority.sources[0],
                            Source::Request(OracleRequest::RedStone(_))
                        ));
                        assert!(matches!(
                            priority.sources[1],
                            Source::Request(OracleRequest::Pyth(_))
                        ));
                        assert!(matches!(
                            priority.sources[2],
                            Source::Request(OracleRequest::Pyth(_))
                        ));
                    }
                    other => panic!("unexpected aggregator: {other:?}"),
                }
            }
            other => panic!("unexpected operation: {other:?}"),
        }
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

        assert!(matches!(proxy.aggregator, Aggregator::Priority(_)));
    }
}
