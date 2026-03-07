use near_sdk::near;

use crate::oracle::pyth;

fn weighted_median_low<T>(sorted_weighted_items: &[(T, u32)]) -> usize {
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
    pub confidence: Confidence,
    pub sample: Sample,
}

impl Aggregator {
    pub fn aggregate(&self, prices: &[(pyth::Price, u32)]) -> SpecificPrice {
        match &self.sample {
            Sample::MedianLow => {
                let mut values = prices
                    .iter()
                    .flat_map(|(price, weight)| {
                        // Split apart prices so that we don't need to worry about confidence when sorting.
                        let [lower, upper] = SpecificPrice::split(price);
                        [(lower, *weight), (upper, *weight)]
                    })
                    .collect::<Vec<_>>();
                values.sort_unstable();
                values.swap_remove(weighted_median_low(&values)).0
            }
        }
    }
}

#[derive(Debug, Clone, Eq)]
pub struct SpecificPrice {
    pub value: i64,
    pub exponent: i32,
    pub publish_time: i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Confidence {
    MedianLow { ignore_zeros: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Sample {
    MedianLow,
}

#[cfg(test)]
mod tests {
    use super::*;

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
