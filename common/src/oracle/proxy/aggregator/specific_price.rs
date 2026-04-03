use crate::oracle::pyth::{self, PythTimestamp};

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
