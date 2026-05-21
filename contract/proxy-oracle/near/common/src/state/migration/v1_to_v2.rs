use near_sdk::near;
use templar_common::{
    governance::{Governance, Proposal},
    oracle::pyth::PriceIdentifier,
    versioned_state::StateTransformer,
};
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};

use crate::{
    governance::Operation,
    input::Source,
    state::{self, v1, v2},
};

fn snapshot_proposals(governance: &Governance<Operation>) -> Vec<(u32, Proposal<Operation>)> {
    governance
        .proposals
        .iter()
        .map(|(proposal_id, proposal)| (*proposal_id, proposal.clone()))
        .collect()
}

fn snapshot_proxies(
    proxies: &near_sdk::collections::UnorderedMap<PriceIdentifier, Proxy<Source>>,
) -> Vec<(PriceIdentifier, Proxy<Source>)> {
    proxies.iter().collect()
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub struct V1ToV2;

impl StateTransformer for V1ToV2 {
    type Input = v1::State;
    type Output = v2::State;
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

        let mut governance = Governance::new(v2::StorageKey::Governance);
        governance.next_id = next_id;
        governance.ttl = ttl;
        for (proposal_id, proposal) in proposals {
            governance.proposals.insert(proposal_id, proposal);
        }

        let mut proxies = near_sdk::collections::UnorderedMap::new(v2::StorageKey::Proxies);
        let mut circuit_breakers =
            near_sdk::collections::UnorderedMap::new(v2::StorageKey::CircuitBreakers);
        let cached_prices = near_sdk::collections::UnorderedMap::new(v2::StorageKey::CachedPrices);
        for (price_id, proxy) in proxies_snapshot {
            proxies.insert(&price_id, &proxy);
            circuit_breakers.insert(&price_id, &CircuitBreakerSet::empty());
        }

        Ok(state::v2::State {
            governance,
            proxies,
            circuit_breakers,
            cached_prices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use near_sdk::{test_utils::VMContextBuilder, testing_env};
    use templar_common::{
        governance::Governance, oracle::pyth::PriceIdentifier, versioned_state::StateTransformer,
        Nanoseconds,
    };
    use templar_proxy_oracle_kernel::proxy::{Aggregator, FreshnessFilter, Proxy};

    use crate::request::OracleRequest;

    #[test]
    fn v1_to_v2_preserves_proxies_and_initializes_empty_breaker_sets() {
        testing_env!(VMContextBuilder::new().build());

        let price_id = PriceIdentifier([0x11; 32]);
        let proxy = Proxy::new(
            Aggregator::median_low([
                OracleRequest::pyth("pyth.near".parse().unwrap(), price_id).into()
            ]),
            FreshnessFilter::empty(),
        );
        let mut input = v1::State {
            governance: Governance::new(v1::StorageKey::Governance),
            proxies: near_sdk::collections::UnorderedMap::new(v1::StorageKey::Proxies),
        };
        input.governance.ttl = Nanoseconds::from_secs(7);
        input.proxies.insert(&price_id, &proxy);

        let output = V1ToV2.transform(input).unwrap();

        assert_eq!(output.governance.ttl, Nanoseconds::from_secs(7));
        assert_eq!(output.proxies.get(&price_id), Some(proxy));
        assert_eq!(
            output.circuit_breakers.get(&price_id),
            Some(CircuitBreakerSet::empty())
        );
        assert!(output.cached_prices.is_empty());
    }
}
