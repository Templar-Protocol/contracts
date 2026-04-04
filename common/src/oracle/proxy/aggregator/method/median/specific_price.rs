use crate::oracle::pyth::{self, PythTimestamp};

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
        let expo_diff = self.exponent.abs_diff(other.exponent);
        let scale = 10i128.saturating_pow(expo_diff);
        let (lhs, rhs) = if self.exponent >= other.exponent {
            (
                i128::from(self.value).saturating_mul(scale),
                i128::from(other.value),
            )
        } else {
            (
                i128::from(self.value),
                i128::from(other.value).saturating_mul(scale),
            )
        };
        lhs.cmp(&rhs)
    }
}

#[cfg(test)]
mod tests {
    use crate::oracle::pyth::PythTimestamp;

    use super::*;

    fn sp(value: i64, exponent: i32) -> SpecificPrice {
        SpecificPrice {
            value,
            exponent,
            publish_time: PythTimestamp::from_secs(0),
        }
    }

    #[rstest::rstest]
    #[case(sp(100, -4), sp(200, -4), std::cmp::Ordering::Less)]
    #[case(sp(200, -4), sp(200, -4), std::cmp::Ordering::Equal)]
    #[case(sp(300, -4), sp(200, -4), std::cmp::Ordering::Greater)]
    #[case(sp(1, -3), sp(10, -4), std::cmp::Ordering::Equal)]
    #[case(sp(10, -4), sp(1, -3), std::cmp::Ordering::Equal)]
    #[case(sp(1, -3), sp(9, -4), std::cmp::Ordering::Greater)]
    #[case(sp(1, -3), sp(11, -4), std::cmp::Ordering::Less)]
    #[case(sp(-100, -4), sp(-200, -4), std::cmp::Ordering::Greater)]
    #[case(sp(-1, -3), sp(-10, -4), std::cmp::Ordering::Equal)]
    #[case(sp(-1, -3), sp(-9, -4), std::cmp::Ordering::Less)]
    #[case(sp(0, -4), sp(0, 4), std::cmp::Ordering::Equal)]
    #[case(sp(0, -4), sp(1, -4), std::cmp::Ordering::Less)]
    #[case(sp(1, 39), sp(1, 0), std::cmp::Ordering::Greater)]
    #[case(sp(0, 39), sp(1, 0), std::cmp::Ordering::Less)]
    #[case(sp(1, 0), sp(1, 39), std::cmp::Ordering::Less)]
    #[case(sp(1, 38), sp(1, 0), std::cmp::Ordering::Greater)]
    fn specific_price_cmp(
        #[case] a: SpecificPrice,
        #[case] b: SpecificPrice,
        #[case] expected: std::cmp::Ordering,
    ) {
        assert_eq!(a.cmp(&b), expected);
    }
}
