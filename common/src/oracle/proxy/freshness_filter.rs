use near_sdk::near;

use crate::{oracle::pyth, time::Nanoseconds};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct FreshnessFilter {
    /// Maximum age of a price in nanoseconds. If a price is older than this, it will be excluded from proxy resolution.
    pub max_age: Option<Nanoseconds>,
    /// Maximum clock drift in nanoseconds. This is the future-analog of `max_age`.
    pub max_clock_drift: Option<Nanoseconds>,
}

impl FreshnessFilter {
    pub fn accepts(&self, price: &pyth::Price, now: Nanoseconds) -> bool {
        let Some(published) = Nanoseconds::try_from_pyth(price.publish_time) else {
            return false;
        };

        if now >= published {
            self.max_age
                .is_none_or(|max| now.saturating_sub(published) <= max)
        } else {
            self.max_clock_drift
                .is_none_or(|max| published.saturating_sub(now) <= max)
        }
    }
}
