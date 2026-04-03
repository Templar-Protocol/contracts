pub mod aggregator;
pub mod governance;

use near_sdk::near;

use crate::time::Nanoseconds;

use aggregator::{
    filter::Filter,
    method::{
        median_low::{MedianHigh, MedianLow},
        priority::Priority,
        AggregationMethod,
    },
    source::{Source, WeightedSource},
};

use super::pyth;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
#[serde(tag = "aggregator")]
pub enum Proxy {
    MedianLow(MedianLow),
    Priority(Priority),
    MedianHigh(MedianHigh),
}

impl Proxy {
    pub fn median_low(entries: impl IntoIterator<Item = Source>) -> Self {
        Self::MedianLow(MedianLow::new(
            entries.into_iter().map(|s| WeightedSource::new(s, 1)),
            Filter::new(
                Some(Nanoseconds::from_ms(60 * 1000)),
                Some(Nanoseconds::from_ms(10 * 1000)),
            ),
        ))
    }

    pub fn priority(entries: impl IntoIterator<Item = Source>) -> Self {
        Self::Priority(Priority {
            sources: entries.into_iter().collect(),
            filter: Filter::new(
                Some(Nanoseconds::from_ms(60 * 1000)),
                Some(Nanoseconds::from_ms(10 * 1000)),
            ),
        })
    }
}

impl AggregationMethod for Proxy {
    fn sources(&self) -> Vec<&Source> {
        match self {
            Proxy::MedianLow(inner) => inner.sources(),
            Proxy::Priority(inner) => inner.sources(),
            Proxy::MedianHigh(inner) => inner.sources(),
        }
    }

    fn aggregate(
        &self,
        prices: &[Option<pyth::Price>],
        now: Nanoseconds,
    ) -> Result<pyth::Price, aggregator::method::Error> {
        match self {
            Proxy::MedianLow(inner) => inner.aggregate(prices, now),
            Proxy::Priority(inner) => inner.aggregate(prices, now),
            Proxy::MedianHigh(inner) => inner.aggregate(prices, now),
        }
    }
}
