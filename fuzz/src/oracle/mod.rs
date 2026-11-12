//! Exact reference model of the proxy-oracle kernel (`templar-proxy-oracle-kernel`).
//!
//! Every comparison is evaluated with arbitrary-precision integers straight from each rule's
//! definition, so the oracle fuzz targets hold the kernel's fixed-width `U512` arithmetic to an
//! implementation that shares none of it. The kernel resolves any comparison it cannot
//! represent in 512 bits by tripping (fail-closed); `Verdict::Unrepresentable` marks exactly
//! that region.

pub mod breaker_set;
pub mod input;

use core::{cmp::Ordering, slice};

use num_bigint::BigUint;
use templar_primitives::{number::FRACTIONAL_BITS, Decimal, Nanoseconds};
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::method::Error as AggregationError,
        circuit_breaker::{CircuitBreaker, CircuitBreakerSet, UncheckedCircuitBreakerSet},
        Aggregator, FreshnessFilter, WeightedSource,
    },
    Price,
};

const WORD_BITS: usize = 512;
/// Smallest exponent gap `g` with `10^g >= 2^512`: a mantissa scaled across it is unrepresentable.
const UNREPRESENTABLE_EXPONENT_GAP: u32 = 155;
/// Exponent gap at which the larger exponent alone orders two prices, since `10^19 > i64::MAX`.
const DECISIVE_EXPONENT_GAP: u32 = 19;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Exact(bool),
    Unrepresentable,
}

impl Verdict {
    fn trips(self) -> bool {
        match self {
            Self::Exact(trips) => trips,
            Self::Unrepresentable => true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Bound {
    Strict,
    Inclusive,
}

#[derive(Debug, Clone, Copy)]
enum Rank {
    Lower,
    Upper,
}

pub fn is_valid_price(price: &Price) -> bool {
    price.price > 0 && price.price.unsigned_abs() > price.conf
}

fn magnitude(price: &Price) -> BigUint {
    assert!(
        price.price > 0,
        "rules only ever see validated, strictly positive prices: {price:?}"
    );
    BigUint::from(price.price.unsigned_abs())
}

fn repr(value: Decimal) -> BigUint {
    value
        .as_repr()
        .iter()
        .rev()
        .fold(BigUint::default(), |repr, limb| {
            (repr << 64_u32) | BigUint::from(*limb)
        })
}

/// Exact order of two strictly positive prices by value.
pub fn compare(left: &Price, right: &Price) -> Ordering {
    if left.expo.abs_diff(right.expo) >= DECISIVE_EXPONENT_GAP {
        return left.expo.cmp(&right.expo);
    }
    let common = left.expo.min(right.expo);
    let at_common =
        |price: &Price| magnitude(price) * BigUint::from(10_u8).pow(price.expo.abs_diff(common));
    at_common(left).cmp(&at_common(right))
}

/// Each group's summed magnitude at the exponent shared by both groups, or `None` when a
/// mantissa must be scaled across [`UNREPRESENTABLE_EXPONENT_GAP`].
fn scaled_sums(previous: &[Price], current: &[Price]) -> Option<(BigUint, BigUint)> {
    let common = previous
        .iter()
        .chain(current)
        .map(|price| price.expo)
        .min()
        .unwrap_or_default();
    let sum = |prices: &[Price]| {
        prices.iter().try_fold(BigUint::default(), |sum, price| {
            let gap = price.expo.abs_diff(common);
            (gap < UNREPRESENTABLE_EXPONENT_GAP)
                .then(|| sum + magnitude(price) * BigUint::from(10_u8).pow(gap))
        })
    };
    Some((sum(previous)?, sum(current)?))
}

/// Whether `|current - previous| / previous` exceeds (`Strict`) or reaches (`Inclusive`)
/// `threshold`, cross-multiplied in `Decimal`'s fixed point: `|c - p| * 2^128` against
/// `p * repr(threshold)`.
fn relative_change(
    previous: &BigUint,
    current: &BigUint,
    threshold: Decimal,
    bound: Bound,
) -> Verdict {
    let word = BigUint::from(1_u8) << WORD_BITS;
    let difference = if previous >= current {
        previous - current
    } else {
        current - previous
    };
    let scaled_difference = difference << FRACTIONAL_BITS;
    let scaled_threshold = previous * repr(threshold);
    if [previous, current, &scaled_difference, &scaled_threshold]
        .iter()
        .any(|value| **value >= word)
    {
        return Verdict::Unrepresentable;
    }
    Verdict::Exact(match bound {
        Bound::Strict => scaled_difference > scaled_threshold,
        Bound::Inclusive => scaled_difference >= scaled_threshold,
    })
}

fn window_trips(previous: &[Price], current: &[Price], threshold: Decimal, bound: Bound) -> bool {
    scaled_sums(previous, current)
        .map_or(Verdict::Unrepresentable, |(previous, current)| {
            relative_change(&previous, &current, threshold, bound)
        })
        .trips()
}

fn pair_trips(previous: &Price, current: &Price, threshold: Decimal, bound: Bound) -> bool {
    window_trips(
        slice::from_ref(previous),
        slice::from_ref(current),
        threshold,
        bound,
    )
}

/// Whether `rule` trips on `history`, oldest observation first.
pub fn should_trip(rule: &CircuitBreaker, history: &[Price]) -> bool {
    match rule {
        CircuitBreaker::StepwiseChange(rule) => match history {
            [.., previous, current] => {
                pair_trips(previous, current, rule.max_relative_change, Bound::Strict)
            }
            _ => false,
        },
        CircuitBreaker::MonotonicRun(rule) => {
            monotonic_run_trips(rule.max_streak, rule.min_relative_step_change, history)
        }
        CircuitBreaker::WindowedChangeDelta(rule) => windowed_trips(
            rule.window_len,
            rule.lookback_windows,
            rule.max_relative_mean_change,
            history,
        ),
        CircuitBreaker::CumulativeChange(rule) => history.last().is_some_and(|current| {
            pair_trips(
                &rule.baseline,
                current,
                rule.max_relative_change,
                Bound::Strict,
            )
        }),
    }
}

fn significant_direction(step: &[Price], threshold: Decimal) -> Option<Ordering> {
    let [previous, current] = step else {
        return None;
    };
    let direction = compare(current, previous);
    (direction.is_ne() && pair_trips(previous, current, threshold, Bound::Inclusive))
        .then_some(direction)
}

fn monotonic_run_trips(max_streak: u32, threshold: Decimal, history: &[Price]) -> bool {
    let mut directions = history
        .windows(2)
        .rev()
        .map_while(|step| significant_direction(step, threshold));
    let Some(direction) = directions.next() else {
        return false;
    };
    let streak = 1 + directions.take_while(|next| *next == direction).count();
    max_streak > 0 && u32::try_from(streak).map_or(true, |streak| streak >= max_streak)
}

fn windowed_trips(
    window_len: u32,
    lookback_windows: u32,
    threshold: Decimal,
    history: &[Price],
) -> bool {
    let (Ok(window_len), Ok(lookback_windows)) = (
        usize::try_from(window_len),
        usize::try_from(lookback_windows),
    ) else {
        return false;
    };
    if window_len < 2 || lookback_windows == 0 {
        return false;
    }
    let Some(previous_start) = lookback_windows
        .checked_add(1)
        .and_then(|windows| windows.checked_mul(window_len))
        .and_then(|span| history.len().checked_sub(span))
    else {
        return false;
    };
    window_trips(
        &history[previous_start..previous_start + window_len],
        &history[history.len() - window_len..],
        threshold,
        Bound::Strict,
    )
}

/// Whether `rule` can ever trip under a set's configuration; the kernel refuses to install it
/// otherwise.
pub fn is_valid_for(
    rule: &CircuitBreaker,
    sample_interval_ns: Nanoseconds,
    history_len: u32,
) -> bool {
    let is_unit_fraction = |threshold: Decimal| !threshold.is_zero() && threshold <= Decimal::ONE;
    match rule {
        CircuitBreaker::StepwiseChange(rule) => {
            history_len >= 2 && is_unit_fraction(rule.max_relative_change)
        }
        CircuitBreaker::MonotonicRun(rule) => {
            (1..history_len).contains(&rule.max_streak)
                && sample_interval_ns == Nanoseconds::zero()
                && is_unit_fraction(rule.min_relative_step_change)
        }
        CircuitBreaker::WindowedChangeDelta(rule) => {
            rule.window_len >= 2
                && rule.lookback_windows > 0
                && rule
                    .lookback_windows
                    .checked_add(1)
                    .and_then(|windows| windows.checked_mul(rule.window_len))
                    .is_some_and(|span| span <= history_len)
                && is_unit_fraction(rule.max_relative_mean_change)
        }
        CircuitBreaker::CumulativeChange(rule) => {
            history_len >= 1
                && is_valid_price(&rule.baseline)
                && is_unit_fraction(rule.max_relative_change)
        }
    }
}

/// Whether a source price reaches aggregation: a valid confidence interval, published within
/// the filter's age and clock-drift limits.
pub fn survives(filter: &FreshnessFilter, price: &Price, now: Nanoseconds) -> bool {
    let (published, now) = (price.publish_time_ns.as_ns(), now.as_ns());
    let within = |limit: Option<Nanoseconds>, distance: u64| {
        limit.is_none_or(|limit| distance <= limit.as_ns())
    };
    is_valid_price(price)
        && if now >= published {
            within(filter.max_age_ns, now - published)
        } else {
            within(filter.max_clock_drift_ns, published - now)
        }
}

/// Every price `aggregator` may return for `prices`, which have already passed [`survives`].
///
/// Priority admits exactly one; a weighted median admits every endpoint carrying the median's
/// value, since equal values may sort in any order.
pub fn aggregate(
    aggregator: &Aggregator<usize>,
    prices: &[Option<Price>],
) -> Result<Vec<Price>, AggregationError> {
    let source_count = aggregator.sources().len();
    if prices.len() != source_count {
        return Err(AggregationError::LengthMismatch {
            expected: source_count,
            actual: prices.len(),
        });
    }
    match aggregator {
        Aggregator::Priority(_) => prices
            .iter()
            .flatten()
            .next()
            .map(|price| vec![*price])
            .ok_or(AggregationError::TooFewValidSources {
                expected: 1,
                actual: 0,
            }),
        Aggregator::MedianLow(median) => {
            weighted_median(&median.sources, median.min_sources, prices, Rank::Lower)
        }
        Aggregator::MedianHigh(median) => {
            weighted_median(&median.sources, median.min_sources, prices, Rank::Upper)
        }
    }
}

/// A confidence interval as two zero-confidence prices. An upper end past `i64::MAX`
/// saturates, which keeps it inside the interval.
fn interval_endpoints(price: Price) -> [Price; 2] {
    let confidence = i64::try_from(price.conf).unwrap_or(i64::MAX);
    [
        price.price.saturating_sub(confidence),
        price.price.saturating_add(confidence),
    ]
    .map(|mantissa| Price {
        price: mantissa,
        conf: 0,
        ..price
    })
}

fn weighted_median(
    sources: &[WeightedSource<usize>],
    min_sources: u32,
    prices: &[Option<Price>],
    rank: Rank,
) -> Result<Vec<Price>, AggregationError> {
    let mut endpoints = prices
        .iter()
        .zip(sources)
        .filter(|(_, source)| source.weight > 0)
        .filter_map(|(price, source)| price.map(|price| (price, u128::from(source.weight))))
        .flat_map(|(price, weight)| interval_endpoints(price).map(|endpoint| (endpoint, weight)))
        .collect::<Vec<_>>();
    let quorum = usize::try_from(min_sources.max(1)).unwrap_or(usize::MAX);
    let valid_sources = endpoints.len() / 2;
    if valid_sources < quorum {
        return Err(AggregationError::TooFewValidSources {
            expected: quorum,
            actual: valid_sources,
        });
    }
    endpoints.sort_by(|(left, _), (right, _)| compare(left, right));

    let total_weight = endpoints.iter().map(|(_, weight)| weight).sum::<u128>();
    let target = match rank {
        Rank::Lower => total_weight.div_ceil(2),
        Rank::Upper => total_weight / 2 + 1,
    };
    let mut cumulative_weight = 0;
    let Some((median, _)) = endpoints.iter().find(|(_, weight)| {
        cumulative_weight += weight;
        cumulative_weight >= target
    }) else {
        unreachable!("a positive total weight reaches every rank target");
    };
    Ok(endpoints
        .iter()
        .map(|(endpoint, _)| *endpoint)
        .filter(|endpoint| compare(endpoint, median).is_eq())
        .collect())
}

/// Asserts `set` survives both storage codecs: Borsh (NEAR state) bit-exactly, and postcard
/// (Soroban storage) up to `Decimal`'s documented string precision ([`Decimal::near_equal`]).
pub fn assert_storage_round_trips(set: &CircuitBreakerSet) {
    let borsh = borsh::to_vec(set).and_then(|bytes| borsh::from_slice::<CircuitBreakerSet>(&bytes));
    assert_eq!(borsh.ok().as_ref(), Some(set), "Borsh round-trip");

    let postcard = postcard::to_allocvec(set)
        .and_then(|bytes| postcard::from_bytes::<CircuitBreakerSet>(&bytes));
    let decoded = match postcard {
        Ok(decoded) => decoded,
        Err(error) => panic!("postcard failed to round-trip {set:?}: {error}"),
    };
    let mut decoded = UncheckedCircuitBreakerSet::from(decoded);
    for (breaker_id, state) in &mut decoded.breakers {
        if let Some(original) = set.breakers().get(breaker_id) {
            let original = *threshold(&original.breaker);
            let decoded = threshold_mut(&mut state.breaker);
            if decoded.near_equal(original) {
                *decoded = original;
            }
        }
    }
    assert_eq!(
        CircuitBreakerSet::try_from(decoded).as_ref(),
        Ok(set),
        "postcard round-trip"
    );
}

fn threshold(breaker: &CircuitBreaker) -> &Decimal {
    match breaker {
        CircuitBreaker::StepwiseChange(rule) => &rule.max_relative_change,
        CircuitBreaker::MonotonicRun(rule) => &rule.min_relative_step_change,
        CircuitBreaker::WindowedChangeDelta(rule) => &rule.max_relative_mean_change,
        CircuitBreaker::CumulativeChange(rule) => &rule.max_relative_change,
    }
}

fn threshold_mut(breaker: &mut CircuitBreaker) -> &mut Decimal {
    match breaker {
        CircuitBreaker::StepwiseChange(rule) => &mut rule.max_relative_change,
        CircuitBreaker::MonotonicRun(rule) => &mut rule.min_relative_step_change,
        CircuitBreaker::WindowedChangeDelta(rule) => &mut rule.max_relative_mean_change,
        CircuitBreaker::CumulativeChange(rule) => &mut rule.max_relative_change,
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use rstest::rstest;
    use templar_proxy_oracle_kernel::proxy::{
        aggregator::method::median::{MedianHigh, MedianLow},
        circuit_breaker::{CumulativeChange, MonotonicRun, StepwiseChange, WindowedChangeDelta},
    };

    use super::*;

    fn price(mantissa: i64, exponent: i32) -> Price {
        Price {
            price: mantissa,
            conf: 0,
            expo: exponent,
            publish_time_ns: Nanoseconds::zero(),
        }
    }

    fn mantissas(values: &[i64]) -> Vec<Price> {
        values.iter().map(|value| price(*value, 0)).collect()
    }

    fn dec(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap_or_else(|_| panic!("invalid decimal {value}"))
    }

    fn stepwise(threshold: Decimal) -> CircuitBreaker {
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: threshold,
        })
    }

    #[rstest]
    #[case::above_threshold(price(100, 0), price(111, 0), dec("0.1"), true)]
    #[case::decrease_above_threshold(price(100, 0), price(89, 0), dec("0.1"), true)]
    #[case::exactly_at_threshold(price(100, 0), price(150, 0), dec("0.5"), false)]
    #[case::one_unit_past_threshold(price(100, 0), price(151, 0), dec("0.5"), true)]
    #[case::same_value_mixed_exponents(price(1, -3), price(10, -4), Decimal::ZERO, false)]
    #[case::mixed_exponents_above(price(10_000, -2), price(111, 0), dec("0.1"), true)]
    #[case::unrepresentable_gap_fails_closed(price(1, 0), price(1, 155), Decimal::ONE, true)]
    #[case::unrepresentable_threshold_fails_closed(price(2, 0), price(3, 0), Decimal::MAX, true)]
    fn stepwise_change_follows_its_definition(
        #[case] previous: Price,
        #[case] current: Price,
        #[case] threshold: Decimal,
        #[case] expected: bool,
    ) {
        assert_eq!(
            should_trip(&stepwise(threshold), &[previous, current]),
            expected
        );
    }

    #[test]
    fn exponent_gaps_order_by_exponent_once_decisive() {
        assert_eq!(
            compare(&price(1, 19), &price(i64::MAX, 0)),
            Ordering::Greater
        );
        assert_eq!(compare(&price(1, 18), &price(i64::MAX, 0)), Ordering::Less);
        assert_eq!(compare(&price(10, -4), &price(1, -3)), Ordering::Equal);
    }

    #[test]
    fn inclusive_monotonic_step_counts_exactly_at_threshold() {
        let rule = CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 1,
            min_relative_step_change: dec("0.5"),
        });
        assert!(should_trip(&rule, &mantissas(&[100, 150])));
        assert!(!should_trip(&stepwise(dec("0.5")), &mantissas(&[100, 150])));
    }

    #[rstest]
    #[case::two_rises(&[100, 120, 144], true)]
    #[case::direction_change_breaks_streak(&[100, 120, 144, 130], false)]
    #[case::minor_step_breaks_streak(&[100, 120, 121, 146], false)]
    #[case::too_short(&[100], false)]
    fn monotonic_run_counts_the_latest_significant_streak(
        #[case] values: &[i64],
        #[case] expected: bool,
    ) {
        let rule = CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 2,
            min_relative_step_change: dec("0.05"),
        });
        assert_eq!(should_trip(&rule, &mantissas(values)), expected);
    }

    #[rstest]
    #[case::sums_exactly_at_threshold(3, 1, &[100, 100, 100, 150, 150, 150], false)]
    #[case::sums_past_threshold(3, 1, &[100, 100, 100, 150, 150, 151], true)]
    #[case::lookback_offset_skips_a_window(2, 2, &[100, 100, 999, 999, 101, 101], false)]
    #[case::insufficient_history(3, 1, &[100, 100, 100], false)]
    fn windowed_change_compares_window_sums(
        #[case] window_len: u32,
        #[case] lookback_windows: u32,
        #[case] values: &[i64],
        #[case] expected: bool,
    ) {
        let rule = CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
            window_len,
            lookback_windows,
            max_relative_mean_change: dec("0.5"),
        });
        assert_eq!(should_trip(&rule, &mantissas(values)), expected);
    }

    #[test]
    fn cumulative_change_measures_from_its_baseline() {
        let rule = CircuitBreaker::CumulativeChange(CumulativeChange {
            baseline: price(100, 0),
            max_relative_change: dec("0.25"),
        });
        assert!(!should_trip(&rule, &mantissas(&[180, 125])));
        assert!(should_trip(&rule, &mantissas(&[100, 126])));
        assert!(!should_trip(&rule, &[]));
    }

    #[rstest]
    #[case::stepwise_needs_two(stepwise(Decimal::ONE), Nanoseconds::zero(), 1, false)]
    #[case::stepwise_valid(stepwise(Decimal::ONE), Nanoseconds::from_secs(1), 2, true)]
    #[case::zero_threshold(stepwise(Decimal::ZERO), Nanoseconds::zero(), 2, false)]
    #[case::threshold_above_one(stepwise(dec("1.5")), Nanoseconds::zero(), 2, false)]
    #[case::monotonic_needs_unsampled_history(
        CircuitBreaker::MonotonicRun(MonotonicRun { max_streak: 2, min_relative_step_change: Decimal::ONE }),
        Nanoseconds::from_secs(1),
        3,
        false
    )]
    #[case::monotonic_streak_below_history(
        CircuitBreaker::MonotonicRun(MonotonicRun { max_streak: 3, min_relative_step_change: Decimal::ONE }),
        Nanoseconds::zero(),
        3,
        false
    )]
    #[case::windowed_span_fits(
        CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta { window_len: 2, lookback_windows: 2, max_relative_mean_change: Decimal::ONE }),
        Nanoseconds::zero(),
        6,
        true
    )]
    #[case::windowed_span_overflows(
        CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta { window_len: u32::MAX, lookback_windows: 1, max_relative_mean_change: Decimal::ONE }),
        Nanoseconds::zero(),
        u32::MAX,
        false
    )]
    #[case::cumulative_needs_valid_baseline(
        CircuitBreaker::CumulativeChange(CumulativeChange { baseline: Price { conf: 5, ..price(5, 0) }, max_relative_change: Decimal::ONE }),
        Nanoseconds::zero(),
        1,
        false
    )]
    fn validity_follows_rule_requirements(
        #[case] rule: CircuitBreaker,
        #[case] sample_interval_ns: Nanoseconds,
        #[case] history_len: u32,
        #[case] expected: bool,
    ) {
        assert_eq!(
            is_valid_for(&rule, sample_interval_ns, history_len),
            expected
        );
    }

    #[rstest]
    #[case::equal_weights_split(&[1, 1], &[10, 20], 10, 20)]
    #[case::three_sources(&[1, 1, 1], &[30, 10, 20], 20, 20)]
    #[case::heavy_low_source(&[3, 1], &[10, 20], 10, 10)]
    #[case::heavy_high_source(&[1, 3], &[10, 20], 20, 20)]
    #[case::zero_weight_ignored(&[0, 1], &[10, 20], 20, 20)]
    fn weighted_median_selects_the_rank_value(
        #[case] weights: &[u32],
        #[case] values: &[i64],
        #[case] lower: i64,
        #[case] upper: i64,
    ) {
        let sources = || {
            weights
                .iter()
                .enumerate()
                .map(|(index, weight)| WeightedSource::new(index, *weight))
        };
        let prices = values
            .iter()
            .map(|value| Some(price(*value, 0)))
            .collect::<Vec<_>>();
        let select = |aggregator: Aggregator<usize>| {
            aggregate(&aggregator, &prices)
                .ok()
                .and_then(|candidates| candidates.first().map(|price| price.price))
        };
        assert_eq!(
            select(Aggregator::MedianLow(MedianLow::new(sources()))),
            Some(lower)
        );
        assert_eq!(
            select(Aggregator::MedianHigh(MedianHigh::new(sources()))),
            Some(upper)
        );
    }

    #[test]
    fn aggregation_reports_length_and_quorum_errors() {
        let mut median = MedianLow::new([
            WeightedSource::new(0_usize, 1),
            WeightedSource::new(1_usize, 1),
        ]);
        median.min_sources = 2;
        let median = Aggregator::MedianLow(median);
        assert_eq!(
            aggregate(&median, &[None]),
            Err(AggregationError::LengthMismatch {
                expected: 2,
                actual: 1
            })
        );
        assert_eq!(
            aggregate(&median, &[Some(price(1, 0)), None]),
            Err(AggregationError::TooFewValidSources {
                expected: 2,
                actual: 1
            })
        );
        assert_eq!(
            aggregate(&Aggregator::priority([0, 1]), &[None, Some(price(7, 0))]),
            Ok(vec![price(7, 0)])
        );
    }

    #[test]
    fn freshness_limits_are_inclusive() {
        let filter = FreshnessFilter::new(
            Some(Nanoseconds::from_secs(10)),
            Some(Nanoseconds::from_secs(5)),
        );
        let published = |seconds| Price {
            publish_time_ns: Nanoseconds::from_secs(seconds),
            ..price(1, 0)
        };
        let now = Nanoseconds::from_secs(100);
        assert!(survives(&filter, &published(90), now));
        assert!(!survives(&filter, &published(89), now));
        assert!(survives(&filter, &published(105), now));
        assert!(!survives(&filter, &published(106), now));
        assert!(!survives(
            &filter,
            &Price {
                conf: 1,
                ..published(100)
            },
            now
        ));
    }
}
