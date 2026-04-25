pub mod aggregator;
pub mod freshness_filter;

use crate::*;

pub use aggregator::Aggregator;
pub use freshness_filter::FreshnessFilter;

use aggregator::method::Aggregate;
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

    pub fn sources(&self) -> Vec<&S> {
        self.aggregator.sources()
    }

    pub fn resolve(
        &self,
        prices: Vec<Option<Price>>,
        now: Nanoseconds,
    ) -> Result<Price, aggregator::method::Error> {
        let prices = prices
            .into_iter()
            .map(|price| {
                if price
                    .as_ref()
                    .is_some_and(|p| self.freshness_filter.accepts(p, now))
                {
                    price
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        self.aggregator.aggregate(prices)
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::json_types::{I64, U64};
    use rstest::rstest;
    use templar_common::{
        oracle::pyth::{self, PythTimestamp},
        time::Nanoseconds,
    };

    use crate::{
        proxy::{
            aggregator::method::{median::MedianLow, Error},
            Aggregator, FreshnessFilter, Proxy, WeightedSource,
        },
        request::OracleRequest,
    };

    fn price(value: i64, conf: u64, publish_time: PythTimestamp) -> pyth::Price {
        pyth::Price {
            price: I64(value),
            conf: U64(conf),
            expo: -6,
            publish_time,
        }
    }

    fn secs(s: i64) -> PythTimestamp {
        PythTimestamp::from_secs(s)
    }

    fn median_proxy(freshness_filter: FreshnessFilter, min_sources: u32) -> Proxy {
        let mut aggregator = MedianLow::new([
            WeightedSource::new(
                OracleRequest::redstone("oracle.near".parse().unwrap(), "BTC"),
                1,
            ),
            WeightedSource::new(
                OracleRequest::redstone("oracle.near".parse().unwrap(), "BTC"),
                1,
            ),
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
            Some(price(1_000_000, 0, secs(1_000))),
            Some(price(2_000_000, 0, secs(100))),
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
            Some(price(1_000_000, 0, secs(publish_time_s))),
            Some(price(9_999_999, 0, secs(now_s))),
        ];

        let result = proxy.resolve(prices, now).unwrap();

        assert_eq!(result.price.0, if included { 1_000_000 } else { 9_999_999 });
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
            Some(price(1_000_000, 0, secs(publish_time_s))),
            Some(price(9_999_999, 0, secs(now_s))),
        ];

        let result = proxy.resolve(prices, now).unwrap();

        assert_eq!(result.price.0, if included { 1_000_000 } else { 9_999_999 });
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
            Some(price(1_000_000, 0, secs(-1))),
            Some(price(9_999_999, 0, secs(1_000))),
        ];

        let result = proxy.resolve(prices, Nanoseconds::from_ms(1_000)).unwrap();

        assert_eq!(result.price.0, 9_999_999);
    }
}
