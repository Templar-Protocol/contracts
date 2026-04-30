pub mod aggregator;
pub mod freshness_filter;

use crate::*;

use aggregator::method::Aggregate;
pub use aggregator::Aggregator;
pub use freshness_filter::FreshnessFilter;

use templar_primitives::time::Nanoseconds;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WeightedSource<S> {
        pub source: S,
        pub weight: u32,
    }
}

impl<S> WeightedSource<S> {
    pub fn new(source: impl Into<S>, weight: u32) -> Self {
        Self {
            source: source.into(),
            weight,
        }
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Proxy<S> {
        pub aggregator: Aggregator<S>,
        pub freshness_filter: FreshnessFilter,
    }
}

impl<S> Proxy<S> {
    #[must_use]
    pub fn new(aggregator: Aggregator<S>, freshness_filter: FreshnessFilter) -> Self {
        Self {
            aggregator,
            freshness_filter,
        }
    }

    #[must_use]
    pub fn median_low(
        sources: impl IntoIterator<Item = S>,
        freshness_filter: FreshnessFilter,
    ) -> Self {
        Self::new(Aggregator::median_low(sources), freshness_filter)
    }

    #[must_use]
    pub fn priority(
        sources: impl IntoIterator<Item = S>,
        freshness_filter: FreshnessFilter,
    ) -> Self {
        Self::new(Aggregator::priority(sources), freshness_filter)
    }

    #[must_use]
    pub fn with_freshness_filter(mut self, freshness_filter: FreshnessFilter) -> Self {
        self.freshness_filter = freshness_filter;
        self
    }

    pub fn sources(&self) -> aggregator::SourceIter<'_, S> {
        self.aggregator.sources()
    }

    pub fn resolve<I>(
        &self,
        prices: I,
        now: Nanoseconds,
    ) -> Result<Price, aggregator::method::Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
    {
        self.aggregator.aggregate(
            prices
                .into_iter()
                .map(|price| price.filter(|price| self.freshness_filter.accepts(price, now))),
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use rstest::rstest;
    use templar_primitives::Nanoseconds;

    use crate::{
        proxy::{
            aggregator::method::{median::MedianLow, Error},
            Aggregator, FreshnessFilter, Proxy, WeightedSource,
        },
        Price,
    };

    fn price(value: i64, conf: u64, publish_time_s: u64) -> Price {
        Price {
            price: value,
            conf,
            expo: -6,
            publish_time_ns: Nanoseconds::from_secs(publish_time_s),
        }
    }

    fn median_proxy(freshness_filter: FreshnessFilter, min_sources: u32) -> Proxy<&'static str> {
        let mut aggregator = MedianLow::new([
            WeightedSource::new("source-a", 1),
            WeightedSource::new("source-b", 1),
        ]);
        aggregator.min_sources = min_sources;

        Proxy::new(Aggregator::MedianLow(aggregator), freshness_filter)
    }

    #[test]
    fn resolve_applies_min_sources_after_filtering() {
        let proxy = median_proxy(
            FreshnessFilter {
                max_age_ns: Some(Nanoseconds::from_secs(500)),
                max_clock_drift_ns: None,
            },
            2,
        );
        let prices = vec![
            Some(price(1_000_000, 0, 1_000)),
            Some(price(2_000_000, 0, 100)),
        ];

        let error = proxy
            .resolve(prices, Nanoseconds::from_secs(1_000))
            .unwrap_err();

        assert!(matches!(
            error,
            Error::TooFewValidSources {
                expected: 2,
                actual: 1,
            }
        ));
    }

    #[rstest]
    #[case::one_under_included(501, 1000, 500, true)]
    #[case::exactly_at_limit_included(500, 1000, 500, true)]
    #[case::one_over_excluded(499, 1000, 500, false)]
    fn resolve_max_age_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_age_s: u64,
        #[case] included: bool,
    ) {
        let proxy = median_proxy(
            FreshnessFilter {
                max_age_ns: Some(Nanoseconds::from_secs(max_age_s)),
                max_clock_drift_ns: None,
            },
            1,
        );
        let now = Nanoseconds::from_secs(u64::try_from(now_s).unwrap());
        let prices = vec![
            Some(price(1_000_000, 0, u64::try_from(publish_time_s).unwrap())),
            Some(price(9_999_999, 0, u64::try_from(now_s).unwrap())),
        ];

        let result = proxy.resolve(prices, now).unwrap();

        assert_eq!(result.price, if included { 1_000_000 } else { 9_999_999 });
    }

    #[rstest]
    #[case::exactly_at_limit_included(1500, 1000, 500, true)]
    #[case::one_over_excluded(1501, 1000, 500, false)]
    fn resolve_max_clock_drift_boundary(
        #[case] publish_time_s: i64,
        #[case] now_s: i64,
        #[case] max_clock_drift_s: u64,
        #[case] included: bool,
    ) {
        let proxy = median_proxy(
            FreshnessFilter {
                max_age_ns: None,
                max_clock_drift_ns: Some(Nanoseconds::from_secs(max_clock_drift_s)),
            },
            1,
        );
        let now = Nanoseconds::from_secs(u64::try_from(now_s).unwrap());
        let prices = vec![
            Some(price(1_000_000, 0, u64::try_from(publish_time_s).unwrap())),
            Some(price(9_999_999, 0, u64::try_from(now_s).unwrap())),
        ];

        let result = proxy.resolve(prices, now).unwrap();

        assert_eq!(result.price, if included { 1_000_000 } else { 9_999_999 });
    }

    #[test]
    fn resolve_excludes_negative_publish_times() {
        let proxy = median_proxy(
            FreshnessFilter {
                max_age_ns: Some(Nanoseconds::from_ms(500)),
                max_clock_drift_ns: None,
            },
            1,
        );
        let prices = vec![
            Some(Price {
                price: 1_000_000,
                conf: 0,
                expo: -6,
                publish_time_ns: Nanoseconds::zero(),
            }),
            Some(price(9_999_999, 0, 1_000)),
        ];

        let result = proxy.resolve(prices, Nanoseconds::from_ms(1_000)).unwrap();

        assert_eq!(result.price, 9_999_999);
    }
}
