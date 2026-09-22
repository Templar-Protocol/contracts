//! Property tests for proxy resolution through the public API.
//!
//! The median invariant under test is ENG-361's "aggregation remains within source bounds":
//! whatever survives freshness, confidence, and weight filtering, the selected price is one of
//! the survivors' confidence-interval endpoints and lies between the lowest and highest of them.

use core::cmp::Ordering;

use proptest::prelude::*;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::method::{
            median::{MedianHigh, MedianLow},
            Error,
        },
        circuit_breaker::{CircuitBreaker, CircuitBreakerSet},
        Aggregator, FreshnessFilter, Proxy, ResolveError, WeightedSource,
    },
    Price,
};

#[derive(Debug, Clone, Copy)]
enum Variant {
    Low,
    High,
}

#[derive(Debug, Clone, Copy)]
struct SourceInput {
    weight: u32,
    price: Option<Price>,
}

#[derive(Debug, Clone, Copy)]
struct FreshnessSecs {
    max_age: Option<u64>,
    max_clock_drift: Option<u64>,
    now: u64,
}

impl FreshnessSecs {
    fn filter(self) -> FreshnessFilter {
        FreshnessFilter::new(
            self.max_age.map(Nanoseconds::from_secs),
            self.max_clock_drift.map(Nanoseconds::from_secs),
        )
    }

    fn now_ns(self) -> Nanoseconds {
        Nanoseconds::from_secs(self.now)
    }

    fn accepts(self, price: &Price) -> bool {
        let publish_s = price.publish_time_ns.as_secs();
        if self.now >= publish_s {
            self.max_age.is_none_or(|max| self.now - publish_s <= max)
        } else {
            self.max_clock_drift
                .is_none_or(|max| publish_s - self.now <= max)
        }
    }
}

fn price_strategy() -> impl Strategy<Value = Price> {
    (
        -10i64..=1_000_000,
        0u64..=1_200_000,
        -8i32..=2,
        0u64..=2_000,
    )
        .prop_map(|(price, conf, expo, publish_s)| Price {
            price,
            conf,
            expo,
            publish_time_ns: Nanoseconds::from_secs(publish_s),
        })
}

fn source_strategy() -> impl Strategy<Value = SourceInput> {
    (0u32..=3, proptest::option::of(price_strategy()))
        .prop_map(|(weight, price)| SourceInput { weight, price })
}

fn freshness_strategy() -> impl Strategy<Value = FreshnessSecs> {
    (
        proptest::option::of(0u64..=1_000),
        proptest::option::of(0u64..=1_000),
        0u64..=2_000,
    )
        .prop_map(|(max_age, max_clock_drift, now)| FreshnessSecs {
            max_age,
            max_clock_drift,
            now,
        })
}

fn variant_strategy() -> impl Strategy<Value = Variant> {
    prop_oneof![Just(Variant::Low), Just(Variant::High)]
}

fn median_proxy(
    variant: Variant,
    sources: &[SourceInput],
    min_sources: u32,
    freshness: FreshnessFilter,
) -> Proxy<usize> {
    let weighted = sources
        .iter()
        .enumerate()
        .map(|(index, source)| WeightedSource::new(index, source.weight));
    let aggregator = match variant {
        Variant::Low => {
            let mut median = MedianLow::new(weighted);
            median.min_sources = min_sources;
            Aggregator::MedianLow(median)
        }
        Variant::High => {
            let mut median = MedianHigh::new(weighted);
            median.min_sources = min_sources;
            Aggregator::MedianHigh(median)
        }
    };
    Proxy::new(aggregator, freshness)
}

fn is_valid(price: &Price) -> bool {
    u64::try_from(price.price).is_ok_and(|mantissa| mantissa > price.conf)
}

fn survivor(input: SourceInput, freshness: FreshnessSecs) -> Option<Price> {
    input
        .price
        .filter(|price| input.weight > 0 && is_valid(price) && freshness.accepts(price))
}

fn endpoints(price: &Price) -> [Price; 2] {
    let Ok(conf) = i64::try_from(price.conf) else {
        unreachable!("valid confidence is below a positive i64 mantissa");
    };
    let endpoint = |mantissa: i64| Price {
        price: mantissa,
        conf: 0,
        expo: price.expo,
        publish_time_ns: price.publish_time_ns,
    };
    [endpoint(price.price - conf), endpoint(price.price + conf)]
}

fn compare(left: &Price, right: &Price) -> Ordering {
    let scale = |price: &Price| {
        let shift = u32::try_from(price.expo - left.expo.min(right.expo))
            .unwrap_or_else(|_| unreachable!("exponent is at least the shared minimum"));
        i128::from(price.price) * 10i128.pow(shift)
    };
    scale(left).cmp(&scale(right))
}

proptest! {
    #[test]
    fn median_resolution_selects_a_surviving_endpoint_within_source_bounds(
        variant in variant_strategy(),
        sources in proptest::collection::vec(source_strategy(), 1..=3),
        min_sources in 0u32..=4,
        freshness in freshness_strategy(),
    ) {
        let proxy = median_proxy(variant, &sources, min_sources, freshness.filter());
        let prices = sources.iter().map(|source| source.price).collect::<Vec<_>>();
        let result = proxy.resolve(
            &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
            prices,
            freshness.now_ns(),
        );

        let survivors = sources
            .iter()
            .filter_map(|source| survivor(*source, freshness))
            .collect::<Vec<_>>();
        let quorum = usize::try_from(min_sources.max(1))?;

        if survivors.len() < quorum {
            prop_assert_eq!(
                result,
                Err(ResolveError::Aggregation(Error::TooFewValidSources {
                    expected: quorum,
                    actual: survivors.len(),
                }))
            );
            return Ok(());
        }

        let Ok(outcome) = result else {
            return Err(TestCaseError::fail(format!("{result:?} with survivors {survivors:?}")));
        };
        let Ok(selected) = outcome.value else {
            return Err(TestCaseError::fail(format!("empty breaker set blocked {outcome:?}")));
        };
        let endpoints = survivors.iter().flat_map(endpoints).collect::<Vec<_>>();
        let Some(lowest) = endpoints.iter().min_by(|a, b| compare(a, b)) else {
            unreachable!("quorum guarantees at least one survivor");
        };
        let Some(highest) = endpoints.iter().max_by(|a, b| compare(a, b)) else {
            unreachable!("quorum guarantees at least one survivor");
        };

        prop_assert_eq!(selected.conf, 0);
        prop_assert!(compare(lowest, &selected).is_le(), "{selected:?} below {lowest:?}");
        prop_assert!(compare(&selected, highest).is_le(), "{selected:?} above {highest:?}");
        prop_assert!(
            endpoints.iter().any(|endpoint| compare(endpoint, &selected).is_eq()),
            "{selected:?} is not a surviving endpoint of {survivors:?}"
        );
    }

    #[test]
    fn median_resolution_reports_length_mismatch_exactly(
        variant in variant_strategy(),
        sources in proptest::collection::vec(source_strategy(), 1..=3),
        supplied in proptest::collection::vec(proptest::option::of(price_strategy()), 0..=4),
    ) {
        prop_assume!(supplied.len() != sources.len());
        let proxy = median_proxy(variant, &sources, 1, FreshnessFilter::empty());
        let result = proxy.resolve(
            &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
            supplied.clone(),
            Nanoseconds::zero(),
        );
        prop_assert_eq!(
            result,
            Err(ResolveError::Aggregation(Error::LengthMismatch {
                expected: sources.len(),
                actual: supplied.len(),
            }))
        );
    }
}
