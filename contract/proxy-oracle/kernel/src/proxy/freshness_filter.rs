#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::time::Nanoseconds;

use crate::Price;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FreshnessFilter {
        /// Maximum age of a price in nanoseconds. If a price is older than this, it will be excluded from proxy resolution.
        pub max_age_ns: Option<Nanoseconds>,
        /// Maximum clock drift in nanoseconds. This is the future-analog of `max_age_ns`.
        pub max_clock_drift_ns: Option<Nanoseconds>,
    }
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

    pub fn accepts(&self, price: &Price, now: Nanoseconds) -> bool {
        let published = price.publish_time_ns;

        if now >= published {
            self.max_age_ns
                .is_none_or(|max| now.saturating_sub(published) <= max)
        } else {
            self.max_clock_drift_ns
                .is_none_or(|max| published.saturating_sub(now) <= max)
        }
    }
}

#[cfg(test)]
mod tests {
    use templar_primitives::Nanoseconds;

    use super::*;

    fn price(publish_time_ns: Nanoseconds) -> Price {
        Price {
            price: 1,
            conf: 0,
            expo: 0,
            publish_time_ns,
        }
    }

    #[rstest::rstest]
    #[case::same_time(Nanoseconds::from_secs(1_000))]
    #[case::past_price(Nanoseconds::from_secs(1_001))]
    #[case::future_price(Nanoseconds::from_secs(999))]
    fn accepts_empty_filter(#[case] now: Nanoseconds) {
        let published = Nanoseconds::from_secs(1_000);
        assert!(FreshnessFilter::empty().accepts(&price(published), now));
    }

    #[rstest::rstest]
    #[case::at_publish(Nanoseconds::from_secs(1_000), true)]
    #[case::at_max_age(Nanoseconds::from_secs(1_500), true)]
    #[case::over_max_age(Nanoseconds::from_secs(1_500).saturating_add(Nanoseconds::from_ns(1)), false)]
    fn accepts_respects_max_age_boundaries(#[case] now: Nanoseconds, #[case] accepted: bool) {
        let published = Nanoseconds::from_secs(1_000);
        let max_age = Nanoseconds::from_secs(500);
        let filter = FreshnessFilter::new(Some(max_age), None);

        assert_eq!(filter.accepts(&price(published), now), accepted);
    }

    #[rstest::rstest]
    #[case::at_max_clock_drift(Nanoseconds::from_secs(500), true)]
    #[case::over_max_clock_drift(Nanoseconds::from_secs(499).saturating_sub(Nanoseconds::from_ns(1)), false)]
    fn accepts_respects_max_clock_drift_boundaries(
        #[case] now: Nanoseconds,
        #[case] accepted: bool,
    ) {
        let published = Nanoseconds::from_secs(1_000);
        let max_clock_drift = Nanoseconds::from_secs(500);
        let filter = FreshnessFilter::new(None, Some(max_clock_drift));

        assert_eq!(filter.accepts(&price(published), now), accepted);
    }
}
