mod specific_price;

use std::marker::PhantomData;

use near_sdk::near;
use templar_common::{oracle::pyth, panic_with_message};

use super::Aggregate;
use crate::proxy::{Source, WeightedSource};
use specific_price::SpecificPrice;

/// Calculates the weighted median of a sorted list of weighted items.
///
/// If all of the weights are zero, returns the first item.
///
/// Only definitely correct for lists where `sum(weights)` does not overflow `u32`.
fn median<T>(sorted_weighted_items: &[(T, u32)]) -> (usize, usize) {
    if sorted_weighted_items.len() == 1 {
        return (0, 0);
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Low;

impl MedianVariant for Low {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.min(hi)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct High;

impl MedianVariant for High {
    fn median<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
        let (lo, hi) = median(sorted_weighted_items);
        lo.max(hi)
    }
}

pub type MedianLow = Median<Low>;
pub type MedianHigh = Median<High>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Median<V: MedianVariant> {
    #[serde(skip)]
    #[borsh(skip)]
    _variant: PhantomData<V>,
    pub sources: Vec<WeightedSource>,
    /// Minimum number of sources required for the aggregation to produce a result.
    ///
    /// For example, if the proxy has a Pyth source and a RedStone source, and `min_sources` is set to `2`,
    /// the aggregation will only produce a result if both oracles provide a price.
    pub min_sources: u32,
}

impl<V: MedianVariant> Median<V> {
    pub fn new(sources: impl IntoIterator<Item = WeightedSource>) -> Self {
        Self {
            _variant: PhantomData,
            sources: sources.into_iter().collect(),
            min_sources: 1,
        }
    }
}

impl<V: MedianVariant> Aggregate for Median<V> {
    fn sources(&self) -> Vec<&Source> {
        self.sources.iter().map(|entry| &entry.source).collect()
    }

    fn aggregate(&self, prices: Vec<Option<pyth::Price>>) -> Result<pyth::Price, super::Error> {
        if prices.len() != self.sources.len() {
            panic_with_message("Invariant violation: length mismatch");
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
    use near_sdk::json_types::{I64, U64};
    use templar_common::oracle::pyth::PythTimestamp;

    use crate::{proxy::aggregator::method::Error, request::OracleRequest};

    use super::*;

    fn price(value: i64, conf: u64, publish_time: PythTimestamp) -> pyth::Price {
        pyth::Price {
            price: I64(value),
            conf: U64(conf),
            expo: -6,
            publish_time,
        }
    }

    fn secs(s: i64) -> PythTimestamp {
        PythTimestamp::from_secs(s)
    }

    fn median_low(weights: &[u32], min_sources: u32) -> MedianLow {
        MedianLow {
            _variant: PhantomData,
            sources: weights
                .iter()
                .map(|weight| {
                    WeightedSource::new(
                        OracleRequest::redstone("oracle.near".parse().unwrap(), "BTC"),
                        *weight,
                    )
                })
                .collect(),
            min_sources,
        }
    }

    #[test]
    fn aggregate_empty_returns_too_few_valid_sources() {
        let error = MedianLow::new([]).aggregate(vec![]).unwrap_err();
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
            .aggregate(vec![Some(price(1_000_000, 0, secs(0)))])
            .unwrap();
        assert_eq!(result.price.0, 1_000_000);
    }

    #[test]
    fn aggregate_median_of_three() {
        let prices = vec![
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
            Some(price(3_000_000, 0, secs(0))),
        ];
        let result = median_low(&[1, 1, 1], 1).aggregate(prices).unwrap();
        assert_eq!(result.price.0, 2_000_000);
    }

    #[test]
    fn aggregate_min_sources_not_met_returns_error() {
        let prices = vec![
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
        ];
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
        let prices = vec![
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
        ];
        assert!(median_low(&[1, 1], 2).aggregate(prices).is_ok());
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
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)], "a")]
    #[case(&[("a", 0), ("b", 0), ("c", 0), ("d", 1)], "d")]
    #[case(&[("a", 0), ("b", 1), ("c", 0), ("d", 1)], "b")]
    fn weighted_median_low(#[case] list: &[(&str, u32)], #[case] expected: &str) {
        let item = list[Low::median(list)].0;
        assert_eq!(item, expected);
    }
}
