use near_sdk::collections::UnorderedMap;
use templar_common::governance::{Governance, Proposal};
use templar_common::oracle::pyth::PriceIdentifier;

use crate::proxy::{
    aggregator::method::{median::MedianLow, priority::Priority},
    governance,
    legacy::v0,
    Aggregator, FreshnessFilter, Proxy, ProxyPriceTransformer, Source, WeightedSource,
};

use super::error::MigrationError;

impl From<v0::ProxyPriceTransformer> for ProxyPriceTransformer {
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

impl From<v0::Entry> for WeightedSource {
    fn from(value: v0::Entry) -> Self {
        Self::new(Source::from(value.source), value.weight)
    }
}

impl TryFrom<v0::Filter> for FreshnessFilter {
    type Error = MigrationError;

    fn try_from(value: v0::Filter) -> Result<Self, Self::Error> {
        Ok(Self::new(value.max_age, value.max_clock_drift))
    }
}

impl TryFrom<v0::Proxy> for Proxy {
    type Error = MigrationError;

    fn try_from(value: v0::Proxy) -> Result<Self, Self::Error> {
        let freshness_filter = FreshnessFilter::try_from(value.aggregator.filter.clone())?;

        let aggregator = match value.aggregator.method {
            v0::AggregationMethod::MedianLow => {
                let mut aggregator =
                    MedianLow::new(value.entries.into_iter().map(WeightedSource::from));
                aggregator.min_sources = value.aggregator.filter.min_sources.unwrap_or(1).max(1);
                Aggregator::MedianLow(aggregator)
            }
            v0::AggregationMethod::Priority => {
                let min_sources = value.aggregator.filter.min_sources.unwrap_or(1).max(1);
                if min_sources != 1 {
                    return Err(MigrationError::UnsupportedPriorityMinSources(min_sources));
                }

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

        Ok(Proxy::new(aggregator, freshness_filter))
    }
}

impl TryFrom<v0::governance::Operation> for governance::Operation {
    type Error = MigrationError;

    fn try_from(value: v0::governance::Operation) -> Result<Self, Self::Error> {
        Ok(match value {
            v0::governance::Operation::SetProxy { id, proxy } => Self::SetProxy {
                id,
                proxy: proxy.map(Proxy::try_from).transpose()?,
            },
            v0::governance::Operation::SetActionTtl { new_ttl } => Self::SetActionTtl { new_ttl },
        })
    }
}

pub fn migrate_proposal(
    proposal: Proposal<v0::governance::Operation>,
) -> Result<Proposal<governance::Operation>, MigrationError> {
    Ok(Proposal {
        operation: proposal.operation.try_into()?,
        created_at: proposal.created_at,
        ttl: proposal.ttl,
        created_by: proposal.created_by,
    })
}

pub fn snapshot_proposals(
    governance: &Governance<v0::governance::Operation>,
) -> Result<Vec<(u32, Proposal<governance::Operation>)>, MigrationError> {
    (0..governance.next_id)
        .filter_map(|proposal_id| {
            governance
                .proposals
                .get(&proposal_id)
                .cloned()
                .map(|proposal| migrate_proposal(proposal).map(|proposal| (proposal_id, proposal)))
        })
        .collect()
}

pub fn snapshot_proxies(
    proxies: &UnorderedMap<PriceIdentifier, v0::Proxy>,
) -> Result<Vec<(PriceIdentifier, Proxy)>, MigrationError> {
    proxies
        .iter()
        .map(|(price_id, proxy)| Proxy::try_from(proxy).map(|proxy| (price_id, proxy)))
        .collect()
}
