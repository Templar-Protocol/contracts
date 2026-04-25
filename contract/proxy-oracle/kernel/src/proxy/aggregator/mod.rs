pub mod method;

use crate::*;
use method::{
    median::{MedianHigh, MedianLow},
    priority::Priority,
    Aggregate,
};

use super::WeightedSource;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Aggregator<S> {
        MedianLow(MedianLow<S>),
        Priority(Priority<S>),
        MedianHigh(MedianHigh<S>),
    }
}

impl<S> Aggregator<S> {
    pub fn median_low(entries: impl IntoIterator<Item = S>) -> Self {
        Self::MedianLow(MedianLow::new(
            entries.into_iter().map(|s| WeightedSource::new(s, 1)),
        ))
    }

    pub fn priority(entries: impl IntoIterator<Item = S>) -> Self {
        Self::Priority(Priority::new(entries))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::MedianLow(_) => "MedianLow",
            Self::Priority(_) => "Priority",
            Self::MedianHigh(_) => "MedianHigh",
        }
    }

    pub fn sources(&self) -> Vec<&S> {
        <Self as Aggregate<S>>::sources(self)
    }
}

impl<S> Aggregate<S> for Aggregator<S> {
    fn sources(&self) -> Vec<&S> {
        match self {
            Self::MedianLow(inner) => inner.sources(),
            Self::Priority(inner) => inner.sources(),
            Self::MedianHigh(inner) => inner.sources(),
        }
    }

    fn aggregate(&self, prices: Vec<Option<Price>>) -> Result<Price, method::Error> {
        match self {
            Self::MedianLow(inner) => inner.aggregate(prices),
            Self::Priority(inner) => inner.aggregate(prices),
            Self::MedianHigh(inner) => inner.aggregate(prices),
        }
    }
}
