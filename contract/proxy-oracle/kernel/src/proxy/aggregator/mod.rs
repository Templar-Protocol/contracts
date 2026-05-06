pub mod method;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::format;
#[cfg(feature = "schemars")]
use alloc::{boxed::Box, vec};

use crate::Price;
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

pub enum SourceIter<'a, S> {
    Weighted(core::slice::Iter<'a, WeightedSource<S>>),
    Plain(core::slice::Iter<'a, S>),
}

impl<'a, S> Iterator for SourceIter<'a, S> {
    type Item = &'a S;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Weighted(iter) => iter.next().map(|entry| &entry.source),
            Self::Plain(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Weighted(iter) => iter.size_hint(),
            Self::Plain(iter) => iter.size_hint(),
        }
    }
}

impl<S> ExactSizeIterator for SourceIter<'_, S> {}

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

    pub fn sources(&self) -> SourceIter<'_, S> {
        match self {
            Self::MedianLow(inner) => SourceIter::Weighted(inner.sources.iter()),
            Self::Priority(inner) => SourceIter::Plain(inner.sources.iter()),
            Self::MedianHigh(inner) => SourceIter::Weighted(inner.sources.iter()),
        }
    }
}

impl<S> Aggregate<S> for Aggregator<S> {
    fn aggregate<I>(&self, prices: I) -> Result<Price, method::Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
    {
        match self {
            Self::MedianLow(inner) => inner.aggregate(prices),
            Self::Priority(inner) => inner.aggregate(prices),
            Self::MedianHigh(inner) => inner.aggregate(prices),
        }
    }
}
