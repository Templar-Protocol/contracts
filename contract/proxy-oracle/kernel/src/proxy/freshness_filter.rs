use near_sdk::near;
use templar_common::{oracle::pyth, time::Nanoseconds};

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct FreshnessFilter {
    /// Maximum age of a price in nanoseconds. If a price is older than this, it will be excluded from proxy resolution.
    pub max_age_ns: Option<Nanoseconds>,
    /// Maximum clock drift in nanoseconds. This is the future-analog of `max_age_ns`.
    pub max_clock_drift_ns: Option<Nanoseconds>,
}

impl FreshnessFilter {
    #[must_use]
    pub const fn new(
        max_age_ns: Option<Nanoseconds>,
        max_clock_drift_ns: Option<Nanoseconds>,
    ) -> Self {
        Self {
            max_age_ns,
            max_clock_drift_ns,
        }
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self::new(None, None)
    }

    pub fn accepts(&self, price: &pyth::Price, now: Nanoseconds) -> bool {
        let Some(published) = Nanoseconds::try_from_pyth(price.publish_time) else {
            return false;
        };

        if now >= published {
            self.max_age_ns
                .is_none_or(|max| now.saturating_sub(published) <= max)
        } else {
            self.max_clock_drift_ns
                .is_none_or(|max| published.saturating_sub(now) <= max)
        }
    }
}
