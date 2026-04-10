use near_sdk::near;
use templar_common::{governance::Governance, versioned_state::StateTransformer};

use crate::proxy::migration::{snapshot_proposals, snapshot_proxies, MigrationError};
use crate::state::{self, storage::StorageKey, V1};

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct V0;

impl StateTransformer for V0 {
    type Input = state::V0;
    type Output = V1;
    type Error = MigrationError;

    fn transform(&self, mut input: Self::Input) -> Result<Self::Output, Self::Error> {
        let proposals = snapshot_proposals(&input.governance)?;
        let proxies_snapshot = snapshot_proxies(&input.proxies)?;
        let next_id = input.governance.next_id;
        let ttl = input.governance.ttl;

        input.governance.proposals.clear();
        input.governance.proposals.flush();
        input.proxies.clear();
        drop(input);

        let mut governance = Governance::new(StorageKey::Governance);
        governance.next_id = next_id;
        governance.ttl = ttl;

        for (proposal_id, proposal) in proposals {
            governance.proposals.insert(proposal_id, proposal);
        }

        let mut proxies = near_sdk::collections::UnorderedMap::new(StorageKey::Proxies);
        for (price_id, proxy) in proxies_snapshot {
            proxies.insert(&price_id, &proxy);
        }

        Ok(V1 {
            governance,
            proxies,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use near_sdk::{test_utils::VMContextBuilder, testing_env, AccountId};
    use templar_common::{
        governance::{Governance, Proposal},
        oracle::pyth::PriceIdentifier,
        time::Nanoseconds,
        versioned_state::{read_state_version, write_state_version, StateTransformer},
    };

    use crate::proxy::{
        governance::Operation, legacy::v0, migration::MigrationError, Aggregator, FreshnessFilter,
        Proxy, Source,
    };
    use crate::request::OracleRequest;
    use crate::state::{self, migration::v0_to_v1::V0, storage::StorageKey};

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
            "../../../../near/contract/tests/migration/v0_state_patch.borsh"
        ))
        .unwrap();

        for (key, value) in patch {
            near_sdk::env::storage_write(&key, &value);
        }

        let state = near_sdk::env::state_read::<state::V0>().unwrap();
        assert_eq!(state.governance.next_id, 6);
        assert_eq!(state.proxies.len(), 3);
        assert_eq!(state.governance.proposals.iter().count(), 2);
        assert_eq!(state.proxies.iter().count(), 3);
    }

    #[test]
    fn v0_to_v1_migrates_proxy_map_and_governance() {
        context();

        let btc = PriceIdentifier([0x11; 32]);
        let eth = PriceIdentifier([0x22; 32]);
        let governance_id = 7;
        let created_by = account("owner.near");

        let mut old = state::V0 {
            governance: Governance::new(StorageKey::Governance),
            proxies: near_sdk::collections::UnorderedMap::new(StorageKey::Proxies),
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
                min_sources: Some(1),
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
                operation: v0::governance::Operation::SetProxy {
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

        let new = V0.run().unwrap();

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
                        Some(Nanoseconds::from_secs(70)),
                        Some(Nanoseconds::from_secs(20)),
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
    fn v0_to_v1_rejects_priority_min_sources_over_one() {
        context();

        let error = Proxy::try_from(v0::Proxy {
            aggregator: v0::Aggregator::priority(v0::Filter {
                max_age: None,
                max_clock_drift: None,
                min_sources: Some(2),
            }),
            entries: vec![v0::Entry::new(
                OracleRequest::redstone(account("redstone.near"), "BTC"),
                1,
            )],
        })
        .unwrap_err();

        assert_eq!(error, MigrationError::UnsupportedPriorityMinSources(2));
    }
}
