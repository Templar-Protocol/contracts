pub mod filter;
pub mod method;
pub mod source;
pub mod specific_price;
pub mod transformer;

use method::{
    median::{MedianHigh, MedianLow},
    priority::Priority,
    Aggregate,
};
use near_sdk::near;
use source::{Source, WeightedSource};

use crate::oracle::pyth;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Aggregator {
    MedianLow(MedianLow),
    Priority(Priority),
    MedianHigh(MedianHigh),
}

impl Aggregator {
    pub fn median_low(entries: impl IntoIterator<Item = Source>) -> Self {
        Self::MedianLow(MedianLow::new(
            entries.into_iter().map(|s| WeightedSource::new(s, 1)),
        ))
    }

    pub fn priority(entries: impl IntoIterator<Item = Source>) -> Self {
        Self::Priority(Priority::new(entries))
    }
}

impl Aggregate for Aggregator {
    fn sources(&self) -> Vec<&Source> {
        match self {
            Self::MedianLow(inner) => inner.sources(),
            Self::Priority(inner) => inner.sources(),
            Self::MedianHigh(inner) => inner.sources(),
        }
    }

    fn aggregate(&self, prices: Vec<Option<pyth::Price>>) -> Result<pyth::Price, method::Error> {
        match self {
            Self::MedianLow(inner) => inner.aggregate(prices),
            Self::Priority(inner) => inner.aggregate(prices),
            Self::MedianHigh(inner) => inner.aggregate(prices),
        }
    }
}
