pub mod method;

use method::{
    median::{MedianHigh, MedianLow},
    priority::Priority,
    Aggregate,
};
use near_sdk::near;

use crate::oracle::pyth;

use super::{Source, WeightedSource};

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

    pub fn name(&self) -> &'static str {
        match self {
            Self::MedianLow(_) => "MedianLow",
            Self::Priority(_) => "Priority",
            Self::MedianHigh(_) => "MedianHigh",
        }
    }

    pub fn sources(&self) -> Vec<&Source> {
        <Self as Aggregate>::sources(self)
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
