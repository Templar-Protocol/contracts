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

/// Structural admission errors. Historic configuration decoding remains unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    EmptySources,
    DuplicateSource,
    InvalidMinSources {
        min_sources: u32,
        source_count: usize,
    },
    ZeroWeightSource,
}

impl<S: Eq> Aggregator<S> {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let source_count = self.sources().len();
        if source_count == 0 {
            return Err(ConfigError::EmptySources);
        }
        let median_policy = match self {
            Self::MedianLow(median) => Some((
                median.min_sources,
                median.sources.iter().any(|source| source.weight == 0),
            )),
            Self::MedianHigh(median) => Some((
                median.min_sources,
                median.sources.iter().any(|source| source.weight == 0),
            )),
            Self::Priority(_) => None,
        };
        if let Some((min_sources, zero_weight)) = median_policy {
            if min_sources == 0 || u64::from(min_sources) > source_count as u64 {
                return Err(ConfigError::InvalidMinSources {
                    min_sources,
                    source_count,
                });
            }
            if zero_weight {
                return Err(ConfigError::ZeroWeightSource);
            }
        }
        for (index, source) in self.sources().enumerate() {
            if self.sources().take(index).any(|prior| prior == source) {
                return Err(ConfigError::DuplicateSource);
            }
        }
        Ok(())
    }
}

pub enum SourceIter<'a, S> {
    Weighted(core::slice::Iter<'a, WeightedSource<S>>),
    Plain(core::slice::Iter<'a, S>),
}

impl<S> SourceIter<'_, S> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Weighted(iter) => iter.as_slice().is_empty(),
            Self::Plain(iter) => iter.as_slice().is_empty(),
        }
    }
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

    pub fn median_high(entries: impl IntoIterator<Item = S>) -> Self {
        Self::MedianHigh(MedianHigh::new(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_methods_require_nonempty_distinct_descriptors() {
        for aggregator in [
            Aggregator::<u8>::median_low([]),
            Aggregator::median_high([]),
            Aggregator::priority([]),
        ] {
            assert_eq!(aggregator.validate(), Err(ConfigError::EmptySources));
        }
        for aggregator in [
            Aggregator::median_low([1_u8, 1]),
            Aggregator::median_high([1_u8, 1]),
            Aggregator::priority([1_u8, 1]),
        ] {
            assert_eq!(aggregator.validate(), Err(ConfigError::DuplicateSource));
        }
        for aggregator in [
            Aggregator::median_low([1_u8, 2]),
            Aggregator::median_high([1_u8, 2]),
            Aggregator::priority([1_u8, 2]),
        ] {
            assert_eq!(aggregator.validate(), Ok(()));
        }
    }

    #[test]
    fn median_quorum_weights_and_weight_independent_duplicates_are_checked() {
        for min_sources in [0, 3] {
            let mut low =
                MedianLow::<u8>::new([WeightedSource::new(1, 1), WeightedSource::new(2, 1)]);
            let mut high = MedianHigh::<u8>::new(low.sources.clone());
            low.min_sources = min_sources;
            high.min_sources = min_sources;
            for aggregator in [Aggregator::MedianLow(low), Aggregator::MedianHigh(high)] {
                assert_eq!(
                    aggregator.validate(),
                    Err(ConfigError::InvalidMinSources {
                        min_sources,
                        source_count: 2,
                    })
                );
            }
        }
        for (sources, expected) in [
            ([(1_u8, 1), (2, 0)], ConfigError::ZeroWeightSource),
            ([(1_u8, 1), (1, 2)], ConfigError::DuplicateSource),
        ] {
            let sources = sources.map(|(source, weight)| WeightedSource::<u8>::new(source, weight));
            for aggregator in [
                Aggregator::MedianLow(MedianLow::new(sources.clone())),
                Aggregator::MedianHigh(MedianHigh::new(sources)),
            ] {
                assert_eq!(aggregator.validate(), Err(expected));
            }
        }
    }
}
