use core::cmp::Ordering;

use templar_primitives::time::Nanoseconds;

use crate::price::compare_scaled;

#[derive(Debug, Clone, Eq)]
pub struct SpecificPrice {
    pub value: i64,
    pub exponent: i32,
    pub publish_time_ns: Nanoseconds,
}

impl From<SpecificPrice> for crate::Price {
    fn from(specific_price: SpecificPrice) -> Self {
        Self {
            price: specific_price.value,
            conf: 0,
            expo: specific_price.exponent,
            publish_time_ns: specific_price.publish_time_ns,
        }
    }
}

impl SpecificPrice {
    pub fn split(price: &crate::Price) -> (Self, Self) {
        let conf = i64::try_from(price.conf).unwrap_or(i64::MAX);
        (
            Self {
                value: price.price.saturating_sub(conf),
                exponent: price.expo,
                publish_time_ns: price.publish_time_ns,
            },
            Self {
                value: price.price.saturating_add(conf),
                exponent: price.expo,
                publish_time_ns: price.publish_time_ns,
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

#[cfg(test)]
mod tests {
    use templar_primitives::Nanoseconds;

    use super::*;

    fn sp(value: i64, exponent: i32) -> SpecificPrice {
        SpecificPrice {
            value,
            exponent,
            publish_time_ns: Nanoseconds::zero(),
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
