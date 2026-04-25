mod specific_price;

use crate::*;
use core::marker::PhantomData;

use super::Aggregate;
use crate::proxy::WeightedSource;
use specific_price::SpecificPrice;

/// Calculates the weighted median of a sorted list of weighted items.
///
/// If all of the weights are equal (including zero), returns ordinary positional median.
///
/// Only definitely correct for lists where `sum(weights)` does not overflow `u32`.
///
/// # Panics
///
/// If the list is empty.
fn median<T>(sorted_weighted_items: &[(T, u32)]) -> (usize, usize) {
    if sorted_weighted_items.len() == 1 {
        return (0, 0);
    }

    // case: all weights are equal (including zero)
    let first_weight = sorted_weighted_items[0].1;
    if sorted_weighted_items[1..]
        .iter()
        .all(|(_, weight)| *weight == first_weight)
    {
        let hi = sorted_weighted_items.len() / 2;
        let lo = sorted_weighted_items.len().saturating_sub(1) / 2;
        return (lo, hi);
    }

    // case: weights are different
    let mut lo = 0;
    let mut hi = sorted_weighted_items.len() - 1;
    let mut acc: u32 = 0;

    while lo < hi {
        acc = acc.saturating_add(sorted_weighted_items[lo].1);
        lo += 1;

        while acc >= sorted_weighted_items[hi].1 && hi != 0 {
            acc = acc.saturating_sub(sorted_weighted_items[hi].1);
            hi -= 1;
        }
    }

    (lo, hi)
}

pub trait MedianVariant {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize;
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Low;
}

impl MedianVariant for Low {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.min(hi)
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct High;
}

impl MedianVariant for High {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.max(hi)
    }
}

pub type MedianLow<S> = Median<Low, S>;
pub type MedianHigh<S> = Median<High, S>;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Median<V: MedianVariant, S> {
        #[cfg_attr(feature = "serde", serde(skip))]
        #[cfg_attr(feature = "borsh", borsh(skip))]
        _variant: PhantomData<V>,
        pub sources: Vec<WeightedSource<S>>,
        /// Minimum number of sources required for the aggregation to produce a result.
        ///
        /// For example, if the proxy has a Pyth source and a RedStone source, and `min_sources` is set to `2`,
        /// the aggregation will only produce a result if both oracles provide a price.
        pub min_sources: u32,
    }
}

impl<V: MedianVariant, S> Median<V, S> {
    pub fn new(sources: impl IntoIterator<Item = WeightedSource<S>>) -> Self {
        Self {
            _variant: PhantomData,
            sources: sources.into_iter().collect(),
            min_sources: 1,
        }
    }
}

impl<V: MedianVariant, S> Aggregate<S> for Median<V, S> {
    fn sources(&self) -> Vec<&S> {
        self.sources.iter().map(|entry| &entry.source).collect()
    }

    fn aggregate(&self, prices: Vec<Option<Price>>) -> Result<Price, super::Error> {
        if prices.len() != self.sources.len() {
            return Err(super::Error::LengthMismatch {
                expected: self.sources.len(),
                actual: prices.len(),
            });
        }

        let min_sources = self.min_sources.max(1);
        let valid_sources = prices.iter().filter(|price| price.is_some()).count();

        if valid_sources < min_sources as usize {
            return Err(super::Error::TooFewValidSources {
                expected: min_sources as usize,
                actual: valid_sources,
            });
        }

        let mut values = prices
            .into_iter()
            .zip(&self.sources)
            .filter_map(|(price, source)| price.map(|price| (price, source)))
            .flat_map(|(price, source)| {
                // Split apart prices so that we don't need to worry about confidence when sorting.
                let (lower, upper) = SpecificPrice::split(&price);
                [(lower, source.weight), (upper, source.weight)]
            })
            .collect::<Vec<_>>();

        values.sort_unstable();

        Ok(values.swap_remove(V::median(&values)).0.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::proxy::aggregator::method::Error;

    use super::*;

    fn price(value: i64, conf: u64, publish_time_s: u64) -> Price {
        Price {
            price: value,
            conf,
            expo: -6,
            publish_time_ns: templar_primitives::Nanoseconds::from_secs(publish_time_s),
        }
    }

    fn median_low(weights: &[u32], min_sources: u32) -> MedianLow<&'static str> {
        MedianLow {
            _variant: PhantomData,
            sources: weights
                .iter()
                .map(|weight| WeightedSource::new("source", *weight))
                .collect(),
            min_sources,
        }
    }

    #[test]
    fn aggregate_empty_returns_too_few_valid_sources() {
        let error = MedianLow::<&'static str>::new([])
            .aggregate(vec![])
            .unwrap_err();
        assert!(matches!(
            error,
            Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn aggregate_single_price_no_conf() {
        let result = median_low(&[1], 1)
            .aggregate(vec![Some(price(1_000_000, 0, 0))])
            .unwrap();
        assert_eq!(result.price, 1_000_000);
    }

    #[test]
    fn aggregate_median_of_three() {
        let prices = vec![
            Some(price(1_000_000, 0, 0)),
            Some(price(2_000_000, 0, 0)),
            Some(price(3_000_000, 0, 0)),
        ];
        let result = median_low(&[1, 1, 1], 1).aggregate(prices).unwrap();
        assert_eq!(result.price, 2_000_000);
    }

    #[test]
    fn aggregate_min_sources_not_met_returns_error() {
        let prices = vec![Some(price(1_000_000, 0, 0)), Some(price(2_000_000, 0, 0))];
        let error = median_low(&[1, 1], 3).aggregate(prices).unwrap_err();
        assert!(matches!(
            error,
            Error::TooFewValidSources {
                expected: 3,
                actual: 2,
            }
        ));
    }

    #[test]
    fn aggregate_min_sources_exactly_met() {
        let prices = vec![Some(price(1_000_000, 0, 0)), Some(price(2_000_000, 0, 0))];
        assert!(median_low(&[1, 1], 2).aggregate(prices).is_ok());
    }

    #[test]
    fn raw_weighted_median_handles_zero_and_simple_edges() {
        assert_eq!(median(&[("a", 0_u32), ("b", 0_u32)]), (0, 1));
        assert_eq!(median(&[("a", 1_u32)]), (0, 0));
        assert_eq!(median(&[("a", 1_u32), ("b", 1_u32), ("c", 1_u32)]), (1, 1));
        assert_eq!(
            median(&[("a", 1_u32), ("b", 100_u32), ("c", 1_u32)]),
            (1, 1)
        );
        assert_eq!(
            median(&[("a", 0_u32), ("b", 0_u32), ("c", 0_u32), ("d", 0_u32)]),
            (1, 2)
        );
    }

    #[rstest::rstest]
    #[case(&[("a", 1)], "a")]
    #[case(&[("a", 1), ("b", 1), ("c", 1)], "b")]
    #[case(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)], "b")]
    #[case(&[("a", 2), ("b", 1), ("c", 1), ("d", 1)], "b")]
    #[case(&[("a", 1), ("b", 1), ("c", 1), ("d", 2)], "c")]
    #[case(&[("a", 10), ("b", 2), ("c", 6), ("d", 2)], "a")]
    #[case(&[("a", 1), ("b", 10000), ("c", 1)], "b")]
    #[case(&[("a", 2), ("b", 1), ("c", 1)], "a")]
    #[case(&[("a", u32::MAX), ("b", u32::MAX), ("c", u32::MAX)], "b")]
    #[case(&[("a", u32::MAX), ("b", 0), ("c", u32::MAX)], "a")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)], "b")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 0), ("e", 0)], "c")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 1)], "d")]
    #[case(&[("a", 0), ("b", 1), ("c", 0), ("d", 1)], "b")]
    fn weighted_median_low(#[case] list: &[(&str, u32)], #[case] expected: &str) {
        let item = list[Low::median(list)].0;
        assert_eq!(item, expected);
    }

    #[rstest::rstest]
    #[case(&[("a", 0), ("b", 0)], "b")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)], "c")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 0), ("e", 0)], "c")]
    fn weighted_median_high_all_zero_uses_upper_middle(
        #[case] list: &[(&str, u32)],
        #[case] expected: &str,
    ) {
        let item = list[High::median(list)].0;
        assert_eq!(item, expected);
    }
}
