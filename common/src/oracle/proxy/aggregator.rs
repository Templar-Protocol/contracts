use near_sdk::near;

use crate::oracle::{
    pyth::{self, PythTimestamp},
    time::Milliseconds,
};

fn weighted_median_low<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
    if sorted_weighted_items.len() == 1 {
        return 0;
    }

    let mut lo = 0;
    let mut hi = sorted_weighted_items.len() - 1;
    let mut acc: u32 = 0;

    while lo < hi {
        acc += sorted_weighted_items[lo].1;
        lo += 1;

        while acc >= sorted_weighted_items[hi].1 && hi != 0 {
            acc -= sorted_weighted_items[hi].1;
            hi -= 1;
        }
    }

    lo.min(hi)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Aggregator {
    pub method: AggregationMethod,
    pub filter: Filter,
}

impl Aggregator {
    pub fn median_low(filter: Filter) -> Self {
        Self {
            method: AggregationMethod::MedianLow,
            filter,
        }
    }

    pub fn priority(filter: Filter) -> Self {
        Self {
            method: AggregationMethod::Priority,
            filter,
        }
    }

    pub fn aggregate(
        &self,
        prices: &[(pyth::Price, u32)],
        now: Milliseconds,
    ) -> Option<SpecificPrice> {
        if prices.len() < self.filter.min_sources.unwrap_or(1).max(1) as usize {
            return None;
        }

        let mut values = prices
            .iter()
            .filter(|p| {
                let Some(published) = Milliseconds::try_from_pyth(p.0.publish_time) else {
                    return false;
                };

                if now >= published {
                    self.filter.max_age.is_none_or(|max| now - published <= max)
                } else {
                    self.filter
                        .max_clock_drift
                        .is_none_or(|max| published - now <= max)
                }
            })
            .flat_map(|(price, weight)| {
                // Split apart prices so that we don't need to worry about confidence when sorting.
                let [lower, upper] = SpecificPrice::split(price);
                [(lower, *weight), (upper, *weight)]
            })
            .collect::<Vec<_>>();

        if values.is_empty() {
            return None;
        }

        match &self.method {
            AggregationMethod::MedianLow => {
                values.sort_unstable();
                Some(values.swap_remove(weighted_median_low(&values)).0)
            }
            AggregationMethod::Priority => {
                let mut highest_weighted_ix = 0;
                for (i, (_p, w)) in values.iter().enumerate().skip(1) {
                    if *w > values[highest_weighted_ix].1 {
                        highest_weighted_ix = i;
                    }
                }
                Some(values.swap_remove(highest_weighted_ix).0)
            }
        }
    }
}

#[derive(Debug, Clone, Eq)]
pub struct SpecificPrice {
    pub value: i64,
    pub exponent: i32,
    pub publish_time: PythTimestamp,
}

impl From<SpecificPrice> for pyth::Price {
    fn from(s: SpecificPrice) -> Self {
        Self {
            price: s.value.into(),
            conf: 0.into(),
            expo: s.exponent,
            publish_time: s.publish_time,
        }
    }
}

impl SpecificPrice {
    pub fn split(price: &pyth::Price) -> [Self; 2] {
        let conf = i64::try_from(price.conf.0).unwrap_or(i64::MAX);
        [
            Self {
                value: price.price.0 - conf,
                exponent: price.expo,
                publish_time: price.publish_time,
            },
            Self {
                value: price.price.0 + conf,
                exponent: price.expo,
                publish_time: price.publish_time,
            },
        ]
    }
}

impl PartialEq for SpecificPrice {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl PartialOrd for SpecificPrice {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpecificPrice {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let expo_diff = self.exponent - other.exponent;
        let (lhs, rhs) = if expo_diff >= 0 {
            let scale = if expo_diff < 39 {
                10i128.pow(expo_diff.unsigned_abs())
            } else {
                i128::MAX
            };
            (
                i128::from(self.value).saturating_mul(scale),
                i128::from(other.value),
            )
        } else {
            let scale = if -expo_diff < 39 {
                10i128.pow((-expo_diff).unsigned_abs())
            } else {
                i128::MAX
            };
            (
                i128::from(self.value),
                i128::from(other.value).saturating_mul(scale),
            )
        };
        lhs.cmp(&rhs)
    }
}

/// Aggregation method for the price oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum AggregationMethod {
    /// Selects the median value from the sources, selecting the lower value
    /// in case of an even number of sources.
    MedianLow,
    /// Selects the value of the source with the highest weight.
    Priority,
}

/// Filter configuration for the aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct Filter {
    /// Maximum age of a price in milliseconds. If a price is older than this, it will be excluded from the aggregation.
    pub max_age: Option<Milliseconds>,
    /// Maximum clock drift in milliseconds. This is the future-analog of `max_age`.
    pub max_clock_drift: Option<Milliseconds>,
    /// Minimum number of sources required for the aggregation to produce a result.
    ///
    /// For example, if the proxy has a Pyth source and a RedStone source, and `min_sources` is set to `Some(2)`,
    /// the aggregation will only produce a result if both oracles provide a price.
    pub min_sources: Option<u32>,
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
#[cfg(test)]
mod tests {
    use near_sdk::json_types::{I64, U64};

    use super::*;

    fn sp(value: i64, exponent: i32) -> SpecificPrice {
        SpecificPrice {
            value,
            exponent,
            publish_time: PythTimestamp::from_secs(0),
        }
    }

    fn secs(s: i64) -> PythTimestamp {
        PythTimestamp::from_secs(s)
    }

    // --- SpecificPrice::cmp ---

    #[rstest::rstest]
    #[test]
    // Same exponent: direct comparison.
    #[case(sp(100, -4), sp(200, -4), std::cmp::Ordering::Less)]
    #[case(sp(200, -4), sp(200, -4), std::cmp::Ordering::Equal)]
    #[case(sp(300, -4), sp(200, -4), std::cmp::Ordering::Greater)]
    // Different exponents, equal real values: 1e-3 == 10e-4.
    #[case(sp(1, -3), sp(10, -4), std::cmp::Ordering::Equal)]
    #[case(sp(10, -4), sp(1, -3), std::cmp::Ordering::Equal)]
    // Different exponents, unequal: 1e-3 vs 9e-4 and 11e-4.
    #[case(sp(1, -3), sp(9, -4), std::cmp::Ordering::Greater)]
    #[case(sp(1, -3), sp(11, -4), std::cmp::Ordering::Less)]
    // Negative values.
    #[case(sp(-100, -4), sp(-200, -4), std::cmp::Ordering::Greater)]
    #[case(sp(-1, -3), sp(-10, -4), std::cmp::Ordering::Equal)]
    #[case(sp(-1, -3), sp(-9, -4), std::cmp::Ordering::Less)]
    // Zero.
    #[case(sp(0, -4), sp(0, 4), std::cmp::Ordering::Equal)]
    #[case(sp(0, -4), sp(1, -4), std::cmp::Ordering::Less)]
    // Large expo_diff (>= 39): saturating_mul kicks in.
    // Any positive value with expo_diff=39 saturates to i128::MAX, dominating any finite rhs.
    #[case(sp(1, 39), sp(1, 0), std::cmp::Ordering::Greater)]
    #[case(sp(0, 39), sp(1, 0), std::cmp::Ordering::Less)]
    #[case(sp(1, 0), sp(1, 39), std::cmp::Ordering::Less)]
    // expo_diff = 38 is the last precise case (10^38 < i128::MAX).
    #[case(sp(1, 38), sp(1, 0), std::cmp::Ordering::Greater)]
    fn specific_price_cmp(
        #[case] a: SpecificPrice,
        #[case] b: SpecificPrice,
        #[case] expected: std::cmp::Ordering,
    ) {
        assert_eq!(a.cmp(&b), expected);
    }

    fn price(value: i64, conf: u64, publish_time: PythTimestamp) -> pyth::Price {
        pyth::Price {
            price: I64(value),
            conf: U64(conf),
            expo: -6,
            publish_time,
        }
    }

    #[test]
    fn aggregate_empty_returns_none() {
        assert!(Aggregator::median_low(Filter::default())
            .aggregate(&[], Milliseconds::zero())
            .is_none());
    }

    #[test]
    fn aggregate_single_price_no_conf() {
        // conf=0 means lower==upper==value, so the median is exactly the price value.
        let result = Aggregator::median_low(Filter::default())
            .aggregate(&[(price(1_000_000, 0, secs(0)), 1)], Milliseconds::zero());
        assert_eq!(result.unwrap().value, 1_000_000);
    }

    #[test]
    fn aggregate_median_of_three() {
        // Three equal-weight prices: median should be the middle value.
        let prices = [
            (price(1_000_000, 0, secs(0)), 1),
            (price(2_000_000, 0, secs(0)), 1),
            (price(3_000_000, 0, secs(0)), 1),
        ];
        let result =
            Aggregator::median_low(Filter::default()).aggregate(&prices, Milliseconds::zero());
        assert_eq!(result.unwrap().value, 2_000_000);
    }

    #[test]
    fn aggregate_min_sources_not_met_returns_none() {
        let filter = Filter {
            min_sources: Some(3),
            ..Default::default()
        };
        let prices = [
            (price(1_000_000, 0, secs(0)), 1),
            (price(2_000_000, 0, secs(0)), 1),
        ];
        assert!(Aggregator::median_low(filter)
            .aggregate(&prices, Milliseconds::zero())
            .is_none());
    }

    #[test]
    fn aggregate_min_sources_exactly_met() {
        let filter = Filter {
            min_sources: Some(2),
            ..Default::default()
        };
        let prices = [
            (price(1_000_000, 0, secs(0)), 1),
            (price(2_000_000, 0, secs(0)), 1),
        ];
        assert!(Aggregator::median_low(filter)
            .aggregate(&prices, Milliseconds::zero())
            .is_some());
    }

    #[rstest::rstest]
    #[test]
    #[case::one_under_included(501, 1000, 500, true)]
    #[case::exactly_at_limit_included(500, 1000, 500, true)]
    #[case::one_over_excluded(499, 1000, 500, false)]
    fn aggregate_max_age_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_age_s: u64,
        #[case] included: bool,
    ) {
        // Use two prices: the one under test plus a fresh anchor so aggregate never returns None.
        let anchor = (price(9_999_999, 0, secs(now_s)), 1);
        let under_test = (price(1_000_000, 0, secs(publish_time_s)), 1);
        let filter = Filter {
            max_age: Some(Milliseconds::from_secs(max_age_s)),
            ..Default::default()
        };
        let result = Aggregator::median_low(filter)
            .aggregate(&[under_test, anchor], Milliseconds::from_secs(now_s as u64))
            .unwrap();
        if included {
            // Median of [1_000_000, 9_999_999] — the lower value wins median_low.
            assert_eq!(result.value, 1_000_000);
        } else {
            // Only the anchor survives filtering.
            assert_eq!(result.value, 9_999_999);
        }
    }

    #[rstest::rstest]
    #[test]
    #[case::exactly_at_limit_included(1500, 1000, 500, true)]
    #[case::one_over_excluded(1501, 1000, 500, false)]
    fn aggregate_max_clock_drift_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_clock_drift_s: u64,
        #[case] included: bool,
    ) {
        let anchor = (price(9_999_999, 0, secs(now_s)), 1);
        let under_test = (price(1_000_000, 0, secs(publish_time_s)), 1);
        let filter = Filter {
            max_clock_drift: Some(Milliseconds::from_secs(max_clock_drift_s)),
            ..Default::default()
        };
        let result = Aggregator::median_low(filter)
            .aggregate(&[under_test, anchor], Milliseconds::from_secs(now_s as u64))
            .unwrap();
        if included {
            assert_eq!(result.value, 1_000_000);
        } else {
            assert_eq!(result.value, 9_999_999);
        }
    }

    #[test]
    fn aggregate_negative_publish_time_excluded() {
        // Negative publish_time can't be converted to u64, so the price is filtered out.
        let anchor = (price(9_999_999, 0, secs(1000)), 1);
        let negative_time = (price(1_000_000, 0, secs(-1)), 1);
        let filter = Filter {
            max_age: Some(Milliseconds::from_ms(500)),
            ..Default::default()
        };
        let result = Aggregator::median_low(filter)
            .aggregate(&[negative_time, anchor], Milliseconds::from_ms(1000))
            .unwrap();
        assert_eq!(result.value, 9_999_999);
    }

    // --- Priority aggregation ---

    #[test]
    fn priority_empty_returns_none() {
        assert!(Aggregator::priority(Filter::default())
            .aggregate(&[], Milliseconds::zero())
            .is_none());
    }

    #[test]
    fn priority_single_price() {
        let result = Aggregator::priority(Filter::default())
            .aggregate(&[(price(1_000_000, 0, secs(0)), 1)], Milliseconds::zero());
        assert_eq!(result.unwrap().value, 1_000_000);
    }

    #[test]
    fn priority_selects_highest_weight() {
        let prices = [
            (price(1_000_000, 0, secs(0)), 1),
            (price(2_000_000, 0, secs(0)), 10),
            (price(3_000_000, 0, secs(0)), 5),
        ];
        let result = Aggregator::priority(Filter::default())
            .aggregate(&prices, Milliseconds::zero())
            .unwrap();
        // Highest weight is 10 → price 2_000_000 (lower bound with conf=0).
        assert_eq!(result.value, 2_000_000);
    }

    #[test]
    fn priority_equal_weights_selects_first() {
        let prices = [
            (price(1_000_000, 0, secs(0)), 5),
            (price(2_000_000, 0, secs(0)), 5),
            (price(3_000_000, 0, secs(0)), 5),
        ];
        let result = Aggregator::priority(Filter::default())
            .aggregate(&prices, Milliseconds::zero())
            .unwrap();
        // All weights equal → first entry wins (lower bound of first price).
        assert_eq!(result.value, 1_000_000);
    }

    #[test]
    fn priority_with_confidence_returns_lower_bound() {
        // conf=100 splits into lower (900) and upper (1100), both weight 10.
        // The lower bound comes first in iteration, so it's selected.
        let prices = [
            (price(1_000, 100, secs(0)), 10),
            (price(2_000, 0, secs(0)), 1),
        ];
        let result = Aggregator::priority(Filter::default())
            .aggregate(&prices, Milliseconds::zero())
            .unwrap();
        assert_eq!(result.value, 1_000 - 100);
    }

    #[test]
    fn priority_respects_max_age_filter() {
        let filter = Filter {
            max_age: Some(Milliseconds::from_secs(500)),
            ..Default::default()
        };
        // High-weight price is stale, low-weight price is fresh.
        let prices = [
            (price(1_000_000, 0, secs(0)), 100), // stale at now=1000, max_age=500
            (price(2_000_000, 0, secs(900)), 1), // fresh
        ];
        let result = Aggregator::priority(filter)
            .aggregate(&prices, Milliseconds::from_secs(1000))
            .unwrap();
        // Stale price filtered out, only fresh one remains.
        assert_eq!(result.value, 2_000_000);
    }

    #[test]
    fn priority_min_sources_not_met_returns_none() {
        let filter = Filter {
            min_sources: Some(3),
            ..Default::default()
        };
        let prices = [
            (price(1_000_000, 0, secs(0)), 10),
            (price(2_000_000, 0, secs(0)), 1),
        ];
        assert!(Aggregator::priority(filter)
            .aggregate(&prices, Milliseconds::zero())
            .is_none());
    }

    // --- weighted_median_low ---

    #[rstest::rstest]
    #[test]
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
    fn test_weighted_median(#[case] list: &[(&str, u32)], #[case] expected: &str) {
        let item = list[weighted_median_low(list)].0;
        assert_eq!(item, expected);
    }
}
