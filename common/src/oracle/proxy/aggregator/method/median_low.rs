use std::marker::PhantomData;

use near_sdk::near;

use crate::{
    oracle::{
        proxy::aggregator::{
            filter::Filter,
            source::{Source, WeightedSource},
            specific_price::SpecificPrice,
        },
        pyth,
    },
    panic_with_message,
    time::Nanoseconds,
};

use super::AggregationMethod;

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
    _variant: PhantomData<V>,
    pub sources: Vec<WeightedSource>,
    pub filter: Filter,
}

impl<V: MedianVariant> Median<V> {
    pub fn new(sources: impl IntoIterator<Item = WeightedSource>, filter: Filter) -> Self {
        Self {
            _variant: PhantomData,
            sources: sources.into_iter().collect(),
            filter,
        }
    }
}

impl<V: MedianVariant> AggregationMethod for Median<V> {
    fn sources(&self) -> Vec<&Source> {
        self.sources.iter().map(|e| &e.source).collect()
    }

    fn aggregate(
        &self,
        prices: &[Option<pyth::Price>],
        now: Nanoseconds,
    ) -> Result<pyth::Price, super::Error> {
        if prices.len() != self.sources.len() {
            panic_with_message("Invariant violation: length mismatch");
        }

        let prices_filtered = self
            .sources
            .iter()
            .zip(prices)
            .filter_map(|(entry, price)| {
                if let Some(price) = price {
                    self.filter
                        .price
                        .apply(price, now)
                        .then_some((price, entry.weight))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let min_sources = self.filter.min_sources.unwrap_or(1).max(1);

        if prices_filtered.len() < min_sources as usize {
            return Err(super::Error::TooFewValidSources {
                expected: min_sources as usize,
                actual: prices_filtered.len(),
            });
        }

        let mut values = prices_filtered
            .into_iter()
            .flat_map(|(price, weight)| {
                // Split apart prices so that we don't need to worry about confidence when sorting.
                let (lower, upper) = SpecificPrice::split(price);
                [(lower, weight), (upper, weight)]
            })
            .collect::<Vec<_>>();

        if values.is_empty() {
            panic_with_message("Invariant violation: must not be empty after splitting");
        }

        values.sort_unstable();

        Ok(values.swap_remove(V::median(&values)).0.into())
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::json_types::{I64, U64};

    use crate::{oracle::pyth::PythTimestamp, oracle::OracleRequest, time::Nanoseconds};

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

    fn median_low(filter: Filter, weights: &[u32]) -> MedianLow {
        MedianLow::new(
            weights.iter().map(|weight| {
                WeightedSource::new(
                    OracleRequest::redstone("oracle.near".parse().unwrap(), "BTC"),
                    *weight,
                )
            }),
            filter,
        )
    }

    #[test]
    fn aggregate_empty_returns_too_few_valid_sources() {
        let error = MedianLow::new([], Filter::default())
            .aggregate(&[], Nanoseconds::zero())
            .unwrap_err();
        assert!(matches!(
            error,
            super::super::Error::TooFewValidSources {
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn aggregate_single_price_no_conf() {
        let result = median_low(Filter::default(), &[1])
            .aggregate(&[Some(price(1_000_000, 0, secs(0)))], Nanoseconds::zero())
            .unwrap();
        assert_eq!(result.price.0, 1_000_000);
    }

    #[test]
    fn aggregate_median_of_three() {
        let prices = [
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
            Some(price(3_000_000, 0, secs(0))),
        ];
        let result = median_low(Filter::default(), &[1, 1, 1])
            .aggregate(&prices, Nanoseconds::zero())
            .unwrap();
        assert_eq!(result.price.0, 2_000_000);
    }

    #[test]
    fn aggregate_min_sources_not_met_returns_error() {
        let filter = Filter {
            min_sources: Some(3),
            ..Default::default()
        };
        let prices = [
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
        ];
        let error = median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::zero())
            .unwrap_err();
        assert!(matches!(
            error,
            super::super::Error::TooFewValidSources {
                expected: 3,
                actual: 2,
            }
        ));
    }

    #[test]
    fn aggregate_min_sources_exactly_met() {
        let filter = Filter {
            min_sources: Some(2),
            ..Default::default()
        };
        let prices = [
            Some(price(1_000_000, 0, secs(0))),
            Some(price(2_000_000, 0, secs(0))),
        ];
        assert!(median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::zero())
            .is_ok());
    }

    #[test]
    fn aggregate_min_sources_applies_after_time_filtering() {
        let filter = Filter {
            price: super::super::super::filter::IndividualPriceFilter {
                max_age: Some(Nanoseconds::from_secs(500)),
                max_clock_drift: None,
            },
            min_sources: Some(2),
        };
        let prices = [
            Some(price(1_000_000, 0, secs(1_000))),
            Some(price(2_000_000, 0, secs(100))),
        ];
        let error = median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::from_secs(1_000))
            .unwrap_err();
        assert!(matches!(
            error,
            super::super::Error::TooFewValidSources {
                expected: 2,
                actual: 1,
            }
        ));
    }

    #[rstest::rstest]
    #[case::one_under_included(501, 1000, 500, true)]
    #[case::exactly_at_limit_included(500, 1000, 500, true)]
    #[case::one_over_excluded(499, 1000, 500, false)]
    fn aggregate_max_age_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_age_s: u64,
        #[case] included: bool,
    ) {
        let now_s_u64 = u64::try_from(now_s).unwrap();
        let filter = Filter {
            price: super::super::super::filter::IndividualPriceFilter {
                max_age: Some(Nanoseconds::from_secs(max_age_s)),
                max_clock_drift: None,
            },
            min_sources: None,
        };
        let prices = [
            Some(price(1_000_000, 0, secs(publish_time_s))),
            Some(price(9_999_999, 0, secs(now_s))),
        ];
        let result = median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::from_secs(now_s_u64))
            .unwrap();
        assert_eq!(result.price.0, if included { 1_000_000 } else { 9_999_999 });
    }

    #[rstest::rstest]
    #[case::exactly_at_limit_included(1500, 1000, 500, true)]
    #[case::one_over_excluded(1501, 1000, 500, false)]
    fn aggregate_max_clock_drift_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_clock_drift_s: u64,
        #[case] included: bool,
    ) {
        let now_s_u64 = u64::try_from(now_s).unwrap();
        let filter = Filter {
            price: super::super::super::filter::IndividualPriceFilter {
                max_age: None,
                max_clock_drift: Some(Nanoseconds::from_secs(max_clock_drift_s)),
            },
            min_sources: None,
        };
        let prices = [
            Some(price(1_000_000, 0, secs(publish_time_s))),
            Some(price(9_999_999, 0, secs(now_s))),
        ];
        let result = median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::from_secs(now_s_u64))
            .unwrap();
        assert_eq!(result.price.0, if included { 1_000_000 } else { 9_999_999 });
    }

    #[test]
    fn aggregate_negative_publish_time_excluded() {
        let filter = Filter {
            price: super::super::super::filter::IndividualPriceFilter {
                max_age: Some(Nanoseconds::from_ms(500)),
                max_clock_drift: None,
            },
            min_sources: None,
        };
        let prices = [
            Some(price(1_000_000, 0, secs(-1))),
            Some(price(9_999_999, 0, secs(1000))),
        ];
        let result = median_low(filter, &[1, 1])
            .aggregate(&prices, Nanoseconds::from_ms(1000))
            .unwrap();
        assert_eq!(result.price.0, 9_999_999);
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
