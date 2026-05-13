pub mod aggregator;
pub mod circuit_breaker;
pub mod freshness_filter;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::{format, string::ToString};

use crate::Price;
use aggregator::method::Aggregate;
pub use aggregator::Aggregator;
use circuit_breaker::{CircuitBreakerRule, CircuitBreakerSet};
pub use freshness_filter::FreshnessFilter;

use templar_primitives::time::Nanoseconds;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WeightedSource<S> {
        pub source: S,
        pub weight: u32,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Aggregation(aggregator::method::Error),
    CircuitBreaker(circuit_breaker::CircuitBreakerError),
}

impl From<aggregator::method::Error> for ResolveError {
    fn from(error: aggregator::method::Error) -> Self {
        Self::Aggregation(error)
    }
}

impl From<circuit_breaker::CircuitBreakerError> for ResolveError {
    fn from(error: circuit_breaker::CircuitBreakerError) -> Self {
        Self::CircuitBreaker(error)
    }
}

impl core::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Aggregation(error) => write!(f, "aggregation failed: {error}"),
            Self::CircuitBreaker(error) => write!(f, "circuit breaker failed: {error}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ResolveError {}

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

    pub fn resolve<I, R>(
        &self,
        circuit_breakers: &mut CircuitBreakerSet<R>,
        prices: I,
        now: Nanoseconds,
    ) -> Result<Price, ResolveError>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
        R: CircuitBreakerRule,
    {
        let price = self.aggregate(prices, now)?;
        circuit_breakers.evaluate(price, now)?;
        Ok(price)
    }

    fn aggregate<I>(&self, prices: I, now: Nanoseconds) -> Result<Price, aggregator::method::Error>
    where
        I: IntoIterator<Item = Option<Price>>,
        I::IntoIter: ExactSizeIterator<Item = Option<Price>>,
    {
        self.aggregator.aggregate(prices.into_iter().map(|price| {
            price
                .filter(Price::has_strictly_positive_confidence_interval)
                .filter(|price| self.freshness_filter.accepts(price, now))
        }))
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use rstest::rstest;
    use templar_primitives::Nanoseconds;

    use crate::{
        proxy::{
            aggregator::method::{median::MedianLow, Error},
            circuit_breaker::{
                CircuitBreaker, CircuitBreakerError, CircuitBreakerSet, CircuitBreakerSetConfig,
                StepwiseChange,
            },
            Aggregator, FreshnessFilter, Proxy, ResolveError, WeightedSource,
        },
        Price,
    };
    use templar_primitives::Decimal;

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

    fn priority_proxy(freshness_filter: FreshnessFilter) -> Proxy<&'static str> {
        Proxy::priority(["source-a", "source-b"], freshness_filter)
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
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                Nanoseconds::from_secs(1_000),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ResolveError::Aggregation(Error::TooFewValidSources {
                expected: 2,
                actual: 1,
            })
        ));
    }

    #[test]
    fn resolve_median_applies_min_sources_after_invalid_price_filtering() {
        let proxy = median_proxy(FreshnessFilter::new(None, None), 2);
        let prices = vec![
            Some(price(1_000_000, 1_000_000, 1_000)),
            Some(price(2_000_000, 0, 1_000)),
        ];

        let error = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                Nanoseconds::from_secs(1_000),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ResolveError::Aggregation(Error::TooFewValidSources {
                expected: 2,
                actual: 1,
            })
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

        let result = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                now,
            )
            .unwrap();

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
            FreshnessFilter::new(None, Some(Nanoseconds::from_secs(max_clock_drift_s))),
            1,
        );
        let now = Nanoseconds::from_secs(u64::try_from(now_s).unwrap());
        let prices = vec![
            Some(price(1_000_000, 0, u64::try_from(publish_time_s).unwrap())),
            Some(price(9_999_999, 0, u64::try_from(now_s).unwrap())),
        ];

        let result = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                now,
            )
            .unwrap();

        assert_eq!(result.price, if included { 1_000_000 } else { 9_999_999 });
    }

    #[rstest]
    #[case(
        FreshnessFilter::new(Some(Nanoseconds::from_secs(500)), None),
        vec![
            Some(price(1_000_000, 0, 100)),
            Some(price(2_000_000, 0, 1_000)),
        ]
    )]
    #[case(
        FreshnessFilter::new(None, Some(Nanoseconds::from_secs(500))),
        vec![
            Some(price(1_000_000, 0, 1_501)),
            Some(price(2_000_000, 0, 1_000)),
        ]
    )]
    fn resolve_priority_skips_filtered_first_source(
        #[case] freshness_filter: FreshnessFilter,
        #[case] prices: Vec<Option<Price>>,
    ) {
        let proxy = priority_proxy(freshness_filter);
        let result = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                Nanoseconds::from_secs(1_000),
            )
            .unwrap();

        assert_eq!(result.price, 2_000_000);
    }

    #[test]
    fn resolve_priority_skips_invalid_first_source() {
        let proxy = priority_proxy(FreshnessFilter::new(None, None));
        let result = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                vec![
                    Some(price(1_000_000, 1_000_000, 1_000)),
                    Some(price(2_000_000, 0, 1_000)),
                ],
                Nanoseconds::from_secs(1_000),
            )
            .unwrap();

        assert_eq!(result.price, 2_000_000);
    }

    #[test]
    fn resolve_excludes_zero_publish_time_when_stale() {
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
            Some(Price {
                price: 9_999_999,
                conf: 0,
                expo: -6,
                publish_time_ns: Nanoseconds::from_ms(1_000),
            }),
        ];

        let result = proxy
            .resolve(
                &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
                prices,
                Nanoseconds::from_ms(1_000),
            )
            .unwrap();

        assert_eq!(result.price, 9_999_999);
    }

    #[test]
    fn resolve_applies_tripped_circuit_breaker_while_persisting_history() {
        let proxy = priority_proxy(FreshnessFilter::new(None, None));
        let mut circuit_breakers = CircuitBreakerSet::new(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 2,
        });
        let breaker_id = 0;
        circuit_breakers
            .add(
                breaker_id,
                CircuitBreaker::StepwiseChange(StepwiseChange {
                    max_relative_change: Decimal::from_u8(1) / 10_u8,
                }),
            )
            .unwrap();
        let now = Nanoseconds::from_secs(1_000);

        proxy
            .resolve(
                &mut circuit_breakers,
                [Some(price(100, 0, 1_000)), None],
                now,
            )
            .unwrap();
        assert!(matches!(
            proxy.resolve(
                &mut circuit_breakers,
                [Some(price(120, 0, 1_000)), None],
                now
            ),
            Err(ResolveError::CircuitBreaker(CircuitBreakerError::BreakerTripped {
                tripped_breaker_ids
            })) if tripped_breaker_ids == vec![breaker_id]
        ));
        assert!(matches!(
            proxy.resolve(
                &mut circuit_breakers,
                [Some(price(130, 0, 1_000)), None],
                now
            ),
            Err(ResolveError::CircuitBreaker(CircuitBreakerError::BreakerTripped {
                tripped_breaker_ids
            })) if tripped_breaker_ids == vec![breaker_id]
        ));

        assert_eq!(circuit_breakers.accepted_history().len(), 1);
        assert_eq!(
            circuit_breakers.accepted_history().as_slice()[0]
                .price
                .price,
            100
        );
        assert_eq!(circuit_breakers.observed_history().len(), 2);
        assert_eq!(
            circuit_breakers.observed_history().as_slice()[0]
                .price
                .price,
            120
        );
        assert_eq!(
            circuit_breakers.observed_history().as_slice()[1]
                .price
                .price,
            130
        );
    }
}
