mod specific_price;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
use alloc::vec::Vec;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::{format, string::ToString};
use core::marker::PhantomData;

use super::Aggregate;
use crate::proxy::WeightedSource;
use crate::Price;
use specific_price::SpecificPrice;

/// Calculates the lower and upper weighted medians of a sorted list.
///
/// Zero-weight items do not contribute to either target when total weight is
/// positive. An all-zero list uses the positional-median fallback.
///
/// # Panics
///
/// If the list is empty.
fn median<T>(sorted_weighted_items: &[(T, u32)]) -> (usize, usize) {
    let total_weight = sorted_weighted_items
        .iter()
        .map(|(_, weight)| u128::from(*weight))
        .sum::<u128>();

    if total_weight == 0 {
        let high = sorted_weighted_items.len() / 2;
        let low = sorted_weighted_items.len().saturating_sub(1) / 2;
        return (low, high);
    }

    let low_target = total_weight.div_ceil(2);
    let high_target = total_weight / 2 + 1;
    let find_target = |target| {
        let mut cumulative = 0u128;
        let Some(index) = sorted_weighted_items.iter().position(|(_, weight)| {
            cumulative += u128::from(*weight);
            cumulative >= target
        }) else {
            unreachable!("positive total weight must reach its target");
        };
        index
    };

    (find_target(low_target), find_target(high_target))
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
    fn aggregate<I>(&self, prices: I) -> Result<Price, super::Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
    {
        let prices = prices.into_iter();
        let actual = prices.len();

        if actual != self.sources.len() {
            return Err(super::Error::LengthMismatch {
                expected: self.sources.len(),
                actual,
            });
        }

        let mut values = Vec::with_capacity(actual.saturating_mul(2));
        let mut valid_sources = 0usize;
        for (price, source) in prices.zip(&self.sources) {
            if let Some(price) = price.filter(|_| source.weight > 0) {
                valid_sources += 1;
                let (lower, upper) = SpecificPrice::split(&price);
                values.push((lower, source.weight));
                values.push((upper, source.weight));
            }
        }

        let min_sources = self.min_sources.max(1);
        if valid_sources < min_sources as usize {
            return Err(super::Error::TooFewValidSources {
                expected: min_sources as usize,
                actual: valid_sources,
            });
        }

        values.sort_unstable();

        Ok(values.swap_remove(V::median(&values)).0.into())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

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

    #[test]
    fn raw_weighted_median_handles_large_cumulative_weight_without_u32_overflow() {
        let list = [
            ("a", u32::MAX - 10),
            ("b", 20),
            ("c", 10),
            ("d", u32::MAX - 5),
        ];

        assert_eq!(Low::median(&list), 1);
        assert_eq!(High::median(&list), 1);
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

    #[test]
    fn zero_weight_sources_do_not_satisfy_quorum() {
        let prices = vec![
            Some(price(100, 0, 0)),
            Some(price(1, 0, 0)),
            Some(price(1, 0, 0)),
        ];
        assert!(matches!(
            median_low(&[1, 0, 0], 3).aggregate(prices),
            Err(Error::TooFewValidSources {
                expected: 3,
                actual: 1
            })
        ));
    }

    #[test]
    fn hal_36_high_median_ignores_minority_and_zero_weight_items() {
        assert_eq!(High::median(&[("a", 2), ("b", 1)]), 0);
        assert_eq!(High::median(&[("a", 1), ("b", 0)]), 0);
    }

    #[test]
    fn weighted_medians_match_repeated_weight_reference() {
        for len in 1_usize..=6 {
            let combinations = (0..len).fold(1_usize, |combinations, _| combinations * 5);
            for encoded in 0..combinations {
                let mut digits = encoded;
                let items = (0..len)
                    .map(|index| {
                        let Ok(weight) = u32::try_from(digits % 5) else {
                            unreachable!("modulo-five remainder fits u32");
                        };
                        digits /= 5;
                        (index, weight)
                    })
                    .collect::<Vec<_>>();
                let repeated = items
                    .iter()
                    .flat_map(|(index, weight)| {
                        core::iter::repeat_n(
                            *index,
                            usize::try_from(*weight)
                                .unwrap_or_else(|_| unreachable!("weight fits usize")),
                        )
                    })
                    .collect::<Vec<_>>();

                if repeated.is_empty() {
                    continue;
                }

                assert_eq!(
                    Low::median(&items),
                    repeated[(repeated.len() - 1) / 2],
                    "{items:?}"
                );
                assert_eq!(
                    High::median(&items),
                    repeated[repeated.len() / 2],
                    "{items:?}"
                );
            }
        }
    }
}
