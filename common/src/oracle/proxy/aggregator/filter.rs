use near_sdk::near;

use crate::{oracle::pyth, time::Nanoseconds};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[near(serializers = [json, borsh])]
pub struct Filter {
    /// Maximum age of a price in nanoseconds. If a price is older than this, it will be excluded from the aggregation.
    pub max_age: Option<Nanoseconds>,
    /// Maximum clock drift in nanoseconds. This is the future-analog of `max_age`.
    pub max_clock_drift: Option<Nanoseconds>,
}

impl Filter {
    pub fn accepts(&self, p: &pyth::Price, now: Nanoseconds) -> bool {
        let Some(published) = Nanoseconds::try_from_pyth(p.publish_time) else {
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
