use std::cmp::Ordering;

use templar_common::oracle::pyth::{self, PythTimestamp};

#[derive(Debug, Clone, Eq)]
pub struct SpecificPrice {
    pub value: i64,
    pub exponent: i32,
    pub publish_time: PythTimestamp,
}

impl From<SpecificPrice> for pyth::Price {
    fn from(specific_price: SpecificPrice) -> Self {
        Self {
            price: specific_price.value.into(),
            conf: 0.into(),
            expo: specific_price.exponent,
            publish_time: specific_price.publish_time,
        }
    }
}

impl SpecificPrice {
    pub fn split(price: &pyth::Price) -> (Self, Self) {
        let conf = i64::try_from(price.conf.0).unwrap_or(i64::MAX);
        (
            Self {
                value: price.price.0.saturating_sub(conf),
                exponent: price.expo,
                publish_time: price.publish_time,
            },
            Self {
                value: price.price.0.saturating_add(conf),
                exponent: price.expo,
                publish_time: price.publish_time,
            },
        )
    }
}

impl PartialEq for SpecificPrice {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd for SpecificPrice {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpecificPrice {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_scaled(self.value, self.exponent, other.value, other.exponent)
    }
}

// Compare signed decimal mantissas exactly without normalizing through a fixed-width scaled
// integer. Once sign is handled, we compare by decimal magnitude (`digits + exponent`) and only
// rescale by the difference in mantissa digit counts, which is bounded for `i64` values. This
// keeps extreme exponent gaps (positive and negative) exact without overflow; see the large
// exponent tests below.
fn compare_scaled(
    left_value: i64,
    left_exponent: i32,
    right_value: i64,
    right_exponent: i32,
) -> Ordering {
    // short circuit easy/zero cases
    match (left_value.cmp(&0), right_value.cmp(&0)) {
        (Ordering::Equal, right_sign) => return right_sign.reverse(),
        (left_sign, Ordering::Equal) => return left_sign,
        (left_sign, right_sign) if left_sign != right_sign => return left_sign,
        _ => {}
    }

    // guaranteed: both sides are non-zero, same-sign

    let negative = left_value.is_negative();
    let left_abs = u128::from(left_value.unsigned_abs());
    let right_abs = u128::from(right_value.unsigned_abs());
    let left_log10 = left_abs.ilog10();
    let right_log10 = right_abs.ilog10();

    let left_scale = i64::from(left_exponent) + i64::from(left_log10);
    let right_scale = i64::from(right_exponent) + i64::from(right_log10);
    let magnitude_order = left_scale.cmp(&right_scale).then_with(|| {
        let max_digits = left_log10.max(right_log10);
        let left_scaled = left_abs * 10u128.pow(max_digits - left_log10);
        let right_scaled = right_abs * 10u128.pow(max_digits - right_log10);
        left_scaled.cmp(&right_scaled)
    });

    if negative {
        magnitude_order.reverse()
    } else {
        magnitude_order
    }
}

#[cfg(test)]
mod tests {
    use templar_common::oracle::pyth::PythTimestamp;

    use super::*;

    fn sp(value: i64, exponent: i32) -> SpecificPrice {
        SpecificPrice {
            value,
            exponent,
            publish_time: PythTimestamp::from_secs(0),
        }
    }

    #[rstest::rstest]
    #[case(sp(100, -4), sp(200, -4), Ordering::Less)]
    #[case(sp(200, -4), sp(200, -4), Ordering::Equal)]
    #[case(sp(300, -4), sp(200, -4), Ordering::Greater)]
    #[case(sp(1, -3), sp(10, -4), Ordering::Equal)]
    #[case(sp(10, -4), sp(1, -3), Ordering::Equal)]
    #[case(sp(1, -3), sp(9, -4), Ordering::Greater)]
    #[case(sp(1, -3), sp(11, -4), Ordering::Less)]
    #[case(sp(-100, -4), sp(-200, -4), Ordering::Greater)]
    #[case(sp(-1, -3), sp(-10, -4), Ordering::Equal)]
    #[case(sp(-1, -3), sp(-9, -4), Ordering::Less)]
    #[case(sp(-1, -3), sp(-11, -4), Ordering::Greater)]
    #[case(sp(0, -4), sp(0, 4), Ordering::Equal)]
    #[case(sp(0, -4), sp(1, -4), Ordering::Less)]
    #[case(sp(0, -4), sp(-1, -4), Ordering::Greater)]
    #[case(sp(-1, -4), sp(0, -4), Ordering::Less)]
    #[case(sp(9, -1), sp(10, -1), Ordering::Less)]
    #[case(sp(10, -1), sp(9, -1), Ordering::Greater)]
    #[case(sp(9, -1), sp(90, -2), Ordering::Equal)]
    #[case(sp(-9, -1), sp(-90, -2), Ordering::Equal)]
    #[case(sp(1, -18), sp(1, -19), Ordering::Greater)]
    #[case(sp(-1, -18), sp(-1, -19), Ordering::Less)]
    #[case(sp(i64::MAX, -18), sp(i64::MAX - 1, -18), Ordering::Greater)]
    #[case(sp(i64::MIN + 1, -18), sp(i64::MIN + 2, -18), Ordering::Less)]
    fn specific_price_cmp(
        #[case] a: SpecificPrice,
        #[case] b: SpecificPrice,
        #[case] expected: Ordering,
    ) {
        assert_eq!(a.cmp(&b), expected);
    }

    #[test]
    fn specific_price_cmp_handles_large_positive_exponent_gaps() {
        for exponent in [38, 39, 1_000] {
            assert_eq!(sp(1, exponent).cmp(&sp(1, 0)), Ordering::Greater);
            assert_eq!(sp(1, 0).cmp(&sp(1, exponent)), Ordering::Less);
            assert_eq!(sp(0, exponent).cmp(&sp(1, 0)), Ordering::Less);
            assert_eq!(sp(-1, exponent).cmp(&sp(-1, 0)), Ordering::Less);
        }

        assert_eq!(sp(i64::MAX, 1_000).cmp(&sp(1, 999)), Ordering::Greater);
        assert_eq!(sp(i64::MIN + 1, 1_000).cmp(&sp(-1, 999)), Ordering::Less);
    }

    #[test]
    fn specific_price_cmp_handles_large_negative_exponent_gaps() {
        for exponent in [-38, -39, -1_000] {
            assert_eq!(sp(1, 0).cmp(&sp(1, exponent)), Ordering::Greater);
            assert_eq!(sp(1, exponent).cmp(&sp(1, 0)), Ordering::Less);
            assert_eq!(sp(0, exponent).cmp(&sp(1, 0)), Ordering::Less);
            assert_eq!(sp(-1, 0).cmp(&sp(-1, exponent)), Ordering::Less);
            assert_eq!(sp(-1, exponent).cmp(&sp(-1, 0)), Ordering::Greater);
        }

        assert_eq!(sp(i64::MAX, 0).cmp(&sp(1, -999)), Ordering::Greater);
        assert_eq!(sp(i64::MIN + 1, 0).cmp(&sp(-1, -999)), Ordering::Less);
    }
}
