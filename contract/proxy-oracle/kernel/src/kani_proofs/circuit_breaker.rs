use templar_primitives::{Decimal, Nanoseconds};

use super::{bounded_i8, bounded_u8, normalized_at_exponent_minus_one, symbolic_price_input};
use crate::{
    proxy::circuit_breaker::{
        CircuitBreakerEvent, CircuitBreakerRule, CircuitBreakerState, CircuitBreakerStatus,
        MonotonicRun, Observation, ProposedPriceAcceptance, RingBuffer, StepwiseChange,
        WindowedChangeDelta,
    },
    Price,
};

struct TripOnSelectedPrice {
    selected: Price,
}

impl CircuitBreakerRule for TripOnSelectedPrice {
    fn should_trip(&self, history: &RingBuffer<Observation>) -> bool {
        history
            .last()
            .is_some_and(|observation| observation.price == self.selected)
    }
}

#[derive(Clone, Copy)]
struct RulePriceInput {
    mantissa: i64,
    exponent: i32,
}

fn symbolic_rule_price_up_to(maximum_mantissa: u8, with_symbolic_exponent: bool) -> RulePriceInput {
    let mantissa = bounded_u8(maximum_mantissa);
    kani::assume(mantissa > 0);
    RulePriceInput {
        mantissa: i64::from(mantissa),
        exponent: if with_symbolic_exponent {
            i32::from(bounded_i8(-1, 1))
        } else {
            0
        },
    }
}

fn symbolic_rule_price(with_symbolic_exponent: bool) -> RulePriceInput {
    symbolic_rule_price_up_to(32, with_symbolic_exponent)
}

fn dyadic_threshold(numerator: u8) -> Decimal {
    Decimal::from_repr([
        0,
        u64::from(numerator & 15).wrapping_shl(60),
        u64::from(numerator >> 4),
        0,
        0,
        0,
        0,
        0,
    ])
}

fn observation(input: RulePriceInput, index: usize) -> Observation {
    let timestamp = Nanoseconds::from_ns(index as u64);
    Observation {
        price: Price {
            price: input.mantissa,
            conf: 0,
            expo: input.exponent,
            publish_time_ns: timestamp,
        },
        observed_at_ns: timestamp,
    }
}

fn history_from_prefix<const N: usize>(
    capacity: u32,
    prices: &[RulePriceInput; N],
    prefix_len: usize,
) -> RingBuffer<Observation> {
    let mut history = RingBuffer::new(capacity);
    for (index, price) in prices.iter().enumerate() {
        if index < prefix_len {
            history.push(observation(*price, index));
        }
    }
    history
}

fn normalized_positive_price(input: RulePriceInput) -> u64 {
    normalized_at_exponent_minus_one(input.mantissa, input.exponent) as u64
}

fn strict_relative_change_reference(
    previous: RulePriceInput,
    current: RulePriceInput,
    threshold_numerator: u8,
) -> bool {
    let previous = normalized_positive_price(previous);
    let current = normalized_positive_price(current);
    previous.abs_diff(current).wrapping_mul(16)
        > u64::from(threshold_numerator).wrapping_mul(previous)
}

fn significant_direction_reference(previous: i64, current: i64, threshold_numerator: u8) -> i8 {
    if previous == current
        || previous.abs_diff(current).wrapping_mul(16)
            < u64::from(threshold_numerator).wrapping_mul(previous as u64)
    {
        0
    } else if current > previous {
        1
    } else {
        -1
    }
}

fn monotonic_suffix_reference(
    prices: &[RulePriceInput; 5],
    prefix_len: usize,
    max_streak: u32,
    threshold_numerator: u8,
) -> bool {
    let retained_len = prefix_len.min(4);
    if max_streak == 0 || retained_len < 2 {
        return false;
    }
    let step_count = retained_len - 1;
    let newest = prefix_len - 1;
    let step = |steps_back: usize| {
        significant_direction_reference(
            prices[newest - steps_back - 1].mantissa,
            prices[newest - steps_back].mantissa,
            threshold_numerator,
        )
    };
    let newest_direction = step(0);
    if newest_direction == 0 || max_streak as usize > step_count {
        return false;
    }
    match max_streak {
        1 => true,
        2 => step(1) == newest_direction,
        3 => step(1) == newest_direction && step(2) == newest_direction,
        _ => false,
    }
}

fn window_sums_reference<const N: usize>(
    prices: &[RulePriceInput; N],
    prefix_len: usize,
    capacity: usize,
    lookback_windows: usize,
) -> Option<(u64, u64)> {
    const WINDOW_LEN: usize = 2;
    let retained_len = prefix_len.min(capacity);
    let required_len = WINDOW_LEN.saturating_mul(lookback_windows.saturating_add(1));
    if retained_len < required_len {
        return None;
    }

    let current_start = prefix_len.saturating_sub(WINDOW_LEN);
    let previous_start = current_start.saturating_sub(lookback_windows.saturating_mul(WINDOW_LEN));
    let previous = (prices[previous_start].mantissa as u64)
        .wrapping_add(prices[previous_start.saturating_add(1)].mantissa as u64);
    let current = (prices[current_start].mantissa as u64)
        .wrapping_add(prices[current_start.saturating_add(1)].mantissa as u64);
    Some((previous, current))
}

fn strict_window_change_reference(sums: Option<(u64, u64)>, threshold_numerator: u8) -> bool {
    sums.is_some_and(|(previous, current)| {
        previous.abs_diff(current).wrapping_mul(16)
            > u64::from(threshold_numerator).wrapping_mul(previous)
    })
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(9)]
fn stepwise_matches_strict_relative_reference() {
    let prices = [
        symbolic_rule_price(true),
        symbolic_rule_price(true),
        symbolic_rule_price(true),
    ];
    let threshold_numerator = bounded_u8(16);
    let history = history_from_prefix(2, &prices, 3);
    let empty_history = RingBuffer::new(2);
    let single_history = history_from_prefix(2, &[prices[0]], 1);
    let rule = StepwiseChange {
        max_relative_change: dyadic_threshold(threshold_numerator),
    };
    let actual = CircuitBreakerRule::should_trip(&rule, &history);
    let empty_actual = CircuitBreakerRule::should_trip(&rule, &empty_history);
    let single_actual = CircuitBreakerRule::should_trip(&rule, &single_history);
    let expected = strict_relative_change_reference(prices[1], prices[2], threshold_numerator);
    let eviction_is_correct = history.len() == 2
        && history
            .get(0)
            .is_some_and(|retained| *retained == observation(prices[1], 1));
    assert!(actual == expected);
    assert!(!empty_actual);
    assert!(!single_actual);
    assert!(eviction_is_correct);

    kani::cover!(actual, "stepwise rule can trip");
    kani::cover!(!actual, "stepwise rule can remain below its threshold");
    kani::cover!(
        !empty_actual && !single_actual,
        "stepwise rule is inert with insufficient history"
    );
    kani::cover!(
        eviction_is_correct,
        "stepwise rule uses history after eviction"
    );
    kani::cover!(
        prices[1].mantissa == 16
            && prices[1].exponent == 0
            && prices[2].mantissa == 20
            && prices[2].exponent == 0
            && threshold_numerator == 4
            && !actual,
        "stepwise threshold equality does not trip"
    );
    kani::cover!(
        prices[1].mantissa == 16
            && prices[1].exponent == 0
            && prices[2].mantissa == 21
            && prices[2].exponent == 0
            && threshold_numerator == 4
            && actual,
        "stepwise change above the threshold trips"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(12)]
fn monotonic_matches_significant_suffix_reference() {
    let prices = [
        RulePriceInput {
            mantissa: 1,
            exponent: 0,
        },
        symbolic_rule_price_up_to(9, false),
        symbolic_rule_price_up_to(9, false),
        symbolic_rule_price_up_to(9, false),
        symbolic_rule_price_up_to(9, false),
    ];
    let threshold_numerator = bounded_u8(16);
    let max_streak = u32::from(bounded_u8(4));
    let history = history_from_prefix(4, &prices, 5);
    let single_history = history_from_prefix(4, &[prices[0]], 1);
    let rule = MonotonicRun {
        max_streak,
        min_relative_step_change: dyadic_threshold(threshold_numerator),
    };
    let actual = CircuitBreakerRule::should_trip(&rule, &history);
    let single_actual = CircuitBreakerRule::should_trip(&rule, &single_history);
    let expected = monotonic_suffix_reference(&prices, 5, max_streak, threshold_numerator);
    let eviction_is_correct = history.len() == 4
        && history
            .get(0)
            .is_some_and(|retained| *retained == observation(prices[1], 1));
    assert!(actual == expected);
    assert!(!single_actual);
    assert!(eviction_is_correct);

    kani::cover!(actual, "monotonic suffix can trip");
    kani::cover!(
        !actual,
        "monotonic suffix can remain below its configured streak"
    );
    kani::cover!(
        !single_actual,
        "monotonic rule is inert at the insufficient-history boundary"
    );
    kani::cover!(
        eviction_is_correct,
        "monotonic rule uses the newest retained suffix after eviction"
    );
    kani::cover!(
        prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 9
            && threshold_numerator == 8
            && max_streak == 2
            && actual,
        "threshold-equal monotonic steps count toward the exact streak"
    );
    kani::cover!(
        prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 6
            && threshold_numerator == 8
            && max_streak == 2
            && !actual,
        "a final equal monotonic step resets the suffix"
    );
    kani::cover!(
        prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 3
            && threshold_numerator == 8
            && max_streak == 2
            && !actual,
        "a direction reversal breaks the monotonic suffix"
    );
    kani::cover!(
        prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 8
            && threshold_numerator == 8
            && max_streak == 2
            && !actual,
        "a minor final step resets the monotonic suffix"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(12)]
fn windowed_matches_strict_mean_reference() {
    let prices = [
        RulePriceInput {
            mantissa: 1,
            exponent: 0,
        },
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
    ];
    let threshold_numerator = bounded_u8(16);
    let history = history_from_prefix(4, &prices, 5);
    let insufficient_history = RingBuffer::new(4);
    let sums = window_sums_reference(&prices, 5, 4, 1);
    let rule = WindowedChangeDelta {
        window_len: 2,
        lookback_windows: 1,
        max_relative_mean_change: dyadic_threshold(threshold_numerator),
    };
    let actual = CircuitBreakerRule::should_trip(&rule, &history);
    let insufficient_actual = CircuitBreakerRule::should_trip(&rule, &insufficient_history);
    let expected = strict_window_change_reference(sums, threshold_numerator);
    let eviction_is_correct = history.len() == 4
        && history
            .get(0)
            .is_some_and(|retained| *retained == observation(prices[1], 1));
    assert!(actual == expected);
    assert!(!insufficient_actual);
    assert!(eviction_is_correct);

    kani::cover!(actual, "windowed mean rule can trip");
    kani::cover!(!actual, "windowed mean rule can remain below its threshold");
    kani::cover!(
        !insufficient_actual,
        "windowed mean rule is inert without enough history"
    );
    kani::cover!(
        eviction_is_correct,
        "windowed mean rule uses retained history after eviction"
    );
    kani::cover!(
        prices[1].mantissa == 4
            && prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 6
            && threshold_numerator == 8
            && !actual,
        "windowed threshold equality does not trip"
    );
    kani::cover!(
        prices[1].mantissa == 4
            && prices[2].mantissa == 4
            && prices[3].mantissa == 6
            && prices[4].mantissa == 7
            && threshold_numerator == 8
            && actual,
        "windowed mean change above the threshold trips"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(12)]
fn windowed_lookback_uses_offset_reference() {
    let prices = [
        RulePriceInput {
            mantissa: 1,
            exponent: 0,
        },
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
        symbolic_rule_price_up_to(8, false),
    ];
    let threshold_numerator = bounded_u8(16);
    let history = history_from_prefix(6, &prices, 7);
    let insufficient_history = RingBuffer::new(6);
    let sums = window_sums_reference(&prices, 7, 6, 2);
    let rule = WindowedChangeDelta {
        window_len: 2,
        lookback_windows: 2,
        max_relative_mean_change: dyadic_threshold(threshold_numerator),
    };
    let actual = CircuitBreakerRule::should_trip(&rule, &history);
    let insufficient_actual = CircuitBreakerRule::should_trip(&rule, &insufficient_history);
    let expected = strict_window_change_reference(sums, threshold_numerator);
    let eviction_is_correct = history.len() == 6
        && history
            .get(0)
            .is_some_and(|retained| *retained == observation(prices[1], 1));
    assert!(actual == expected);
    assert!(!insufficient_actual);
    assert!(eviction_is_correct);

    kani::cover!(actual, "offset windowed rule can trip");
    kani::cover!(
        !actual,
        "offset windowed rule can remain below its threshold"
    );
    kani::cover!(
        !insufficient_actual,
        "offset windowed rule is inert without enough history"
    );
    kani::cover!(
        eviction_is_correct,
        "offset windowed rule uses retained history after eviction"
    );
    kani::cover!(
        prices[1].mantissa == 4
            && prices[2].mantissa == 4
            && prices[3].mantissa == 8
            && prices[4].mantissa == 8
            && prices[5].mantissa == 4
            && prices[6].mantissa == 4
            && threshold_numerator == 8
            && !actual,
        "lookback offset ignores the intervening window"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(8)]
fn armed_breaker_trips_once_and_stays_blocking() {
    const BREAKER_ID: u32 = 7;

    let input = symbolic_price_input();
    kani::assume(input.has_valid_confidence());
    kani::assume(input.publish_time_ns > 0);
    let selected = input.price();
    let now = Nanoseconds::from_ns(u64::from(bounded_u8(32)));
    let armed_after = Nanoseconds::from_ns(u64::from(bounded_u8(32)));
    let is_enforced = kani::any::<bool>();

    let mut state = CircuitBreakerState::new(TripOnSelectedPrice { selected });
    state.status = CircuitBreakerStatus::ArmedAfter {
        timestamp_ns: armed_after,
    };
    state.is_enforced = is_enforced;

    let seed = Observation {
        price: Price {
            price: 1,
            conf: 0,
            expo: 0,
            publish_time_ns: Nanoseconds::from_ns(input.publish_time_ns - 1),
        },
        observed_at_ns: Nanoseconds::zero(),
    };
    let trip_observation = Observation {
        price: selected,
        observed_at_ns: now,
    };
    let mut accepted_history = RingBuffer::new(3);
    accepted_history.push(seed);
    let proposed_acceptance = ProposedPriceAcceptance::new(&accepted_history, trip_observation);

    let event = state.apply_armed_transition(BREAKER_ID, &proposed_acceptance, now);

    if now < armed_after {
        assert_eq!(event, None);
        assert_eq!(
            state.status,
            CircuitBreakerStatus::ArmedAfter {
                timestamp_ns: armed_after,
            }
        );
        assert!(!state.is_blocking());
        kani::cover!(
            true,
            "a breaker armed in the future ignores a tripping price"
        );
        return;
    }

    let tripped_status = CircuitBreakerStatus::Tripped {
        tripped_at_ns: now,
        price_update: trip_observation,
    };
    assert_eq!(
        event,
        Some(CircuitBreakerEvent::Tripped {
            breaker_id: BREAKER_ID,
            tripped_at_ns: now,
            price_update: trip_observation,
            is_enforced,
        })
    );
    assert_eq!(state.status, tripped_status);
    assert_eq!(state.is_blocking(), is_enforced);

    accepted_history.push(trip_observation);
    let repeated_proposed_acceptance =
        ProposedPriceAcceptance::new(&accepted_history, trip_observation);
    assert_eq!(
        state.apply_armed_transition(BREAKER_ID, &repeated_proposed_acceptance, now),
        None
    );
    assert_eq!(state.status, tripped_status);
    assert_eq!(state.is_blocking(), is_enforced);

    kani::cover!(is_enforced, "an enforced trip blocks");
    kani::cover!(!is_enforced, "an unenforced trip records without blocking");
    kani::cover!(
        now == armed_after,
        "a breaker trips at its exact arming boundary"
    );
    kani::cover!(
        selected.conf > 0,
        "a tripping price can carry a confidence interval"
    );
    kani::cover!(
        selected.expo != 0,
        "a tripping price can use a non-zero exponent"
    );
}
