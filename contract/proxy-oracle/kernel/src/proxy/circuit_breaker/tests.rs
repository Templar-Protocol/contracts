use core::str::FromStr;

use alloc::{collections::BTreeMap, vec, vec::Vec};
use rstest::rstest;
#[cfg(all(feature = "borsh", feature = "serde"))]
use std::eprintln;
use templar_primitives::{Decimal, Nanoseconds};

use crate::{primitive::AccountId, Price};

use super::*;

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn price(value: i64) -> Price {
    price_with_expo(value, 0)
}

fn price_with_conf(value: i64, conf: u64) -> Price {
    Price {
        price: value,
        conf,
        expo: 0,
        publish_time_ns: Nanoseconds::zero(),
    }
}

fn price_with_expo(value: i64, expo: i32) -> Price {
    Price {
        price: value,
        conf: 0,
        expo,
        publish_time_ns: Nanoseconds::zero(),
    }
}

fn observation(value: i64) -> Observation {
    observation_with_expo(value, 0)
}

fn observation_with_expo(value: i64, expo: i32) -> Observation {
    Observation {
        price: price_with_expo(value, expo),
        observed_at_ns: Nanoseconds::zero(),
    }
}

fn history(values: impl IntoIterator<Item = i64>) -> RingBuffer<Observation> {
    let observations = values.into_iter().map(observation).collect::<Vec<_>>();
    let mut history = RingBuffer::new(u32::try_from(observations.len()).unwrap());
    for observation in observations {
        history.push(observation);
    }
    history
}

fn breaker_set(sample_interval_ns: Nanoseconds, history_len: u32) -> CircuitBreakerSet {
    CircuitBreakerSet::new(CircuitBreakerSetConfig {
        sample_interval_ns,
        history_len,
    })
}

fn actor_id() -> AccountId {
    let mut bytes = [0; 64];
    let account_id = b"breaker.near";
    bytes[..account_id.len()].copy_from_slice(account_id);
    AccountId::from_bytes(bytes)
}

fn assert_blocked_by(
    result: Result<CircuitBreakerOutcome<PriceAcceptance>, CircuitBreakerError>,
    expected_blocking_breaker_ids: Vec<u32>,
) -> CircuitBreakerOutcome<PriceAcceptance> {
    let acceptance = result.unwrap();
    assert_eq!(
        acceptance.value,
        Err(PriceBlockedReason::BreakerTripped {
            blocking_breaker_ids: expected_blocking_breaker_ids,
        })
    );
    acceptance
}

fn assert_manually_blocked(
    result: Result<CircuitBreakerOutcome<PriceAcceptance>, CircuitBreakerError>,
) -> CircuitBreakerOutcome<PriceAcceptance> {
    let acceptance = result.unwrap();
    assert_eq!(acceptance.value, Err(PriceBlockedReason::ManuallyTripped));
    acceptance
}

#[cfg(all(feature = "borsh", feature = "serde"))]
fn calibration_breaker(index: u32) -> CircuitBreaker {
    match index % 3 {
        0 => CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("10"),
        }),
        1 => CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: u32::MAX,
            min_relative_step_change: Decimal::ZERO,
        }),
        _ => CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
            window_len: 2,
            lookback_windows: 1,
            max_relative_change_delta: dec("10"),
        }),
    }
}

#[cfg(all(feature = "borsh", feature = "serde"))]
fn calibration_set(history_len: u32, breaker_count: u32) -> CircuitBreakerSet {
    let mut set = breaker_set(Nanoseconds::zero(), history_len);
    for i in 0..history_len {
        set.try_accept_price(
            price(i64::from(100 + i)),
            Nanoseconds::from_secs(u64::from(i)),
        )
        .unwrap();
    }
    for breaker_id in 0..breaker_count {
        set.add(breaker_id, calibration_breaker(breaker_id))
            .unwrap();
    }
    set
}

#[cfg(all(feature = "borsh", feature = "serde"))]
#[test]
#[ignore = "prints Borsh and JSON sizes for choosing circuit breaker resource bounds"]
fn calibrate_circuit_breaker_set_serialized_sizes() {
    const HISTORY_LENGTHS: &[u32] = &[0, 1, 2, 4, 8, 16, 32, 64, 128, 256];
    const BREAKER_COUNTS: &[u32] = &[0, 1, 2, 4, 8, 16, 32];

    eprintln!("history_len,breaker_count,borsh_bytes,json_bytes");
    for &history_len in HISTORY_LENGTHS {
        for &breaker_count in BREAKER_COUNTS {
            let set = calibration_set(history_len, breaker_count);
            let borsh_bytes = borsh::to_vec(&set).unwrap().len();
            let json_bytes = serde_json::to_vec(&set).unwrap().len();
            eprintln!("{history_len},{breaker_count},{borsh_bytes},{json_bytes}");
        }
    }
}

#[test]
fn stepwise_change_trips_above_threshold() {
    let breaker = StepwiseChange {
        max_relative_change: dec("0.10"),
    };

    assert!(breaker.should_trip(&history([100, 111])));
    assert!(breaker.should_trip(&history([0, 1])));
    assert!(!breaker.should_trip(&history([100, 109])));
}

#[test]
fn stepwise_change_accounts_for_price_exponent() {
    let breaker = StepwiseChange {
        max_relative_change: dec("0.10"),
    };
    let mut equivalent = RingBuffer::new(2);
    equivalent.push(observation_with_expo(1, -3));
    equivalent.push(observation_with_expo(10, -4));

    let mut changed = RingBuffer::new(2);
    changed.push(observation_with_expo(100, -2));
    changed.push(observation_with_expo(111, -2));

    assert!(!breaker.should_trip(&equivalent));
    assert!(breaker.should_trip(&changed));
}

#[test]
fn monotonic_run_trips_on_same_direction_streak() {
    let breaker = MonotonicRun {
        max_streak: 3,
        min_relative_step_change: Decimal::ZERO,
    };

    assert!(breaker.should_trip(&history([100, 101, 102, 103])));
    assert!(breaker.should_trip(&history([0, 1, 2, 3])));
    assert!(!breaker.should_trip(&history([100, 101, 100, 101])));
}

#[test]
fn monotonic_run_accounts_for_price_exponent() {
    let breaker = MonotonicRun {
        max_streak: 2,
        min_relative_step_change: Decimal::ZERO,
    };
    let mut equivalent = RingBuffer::new(3);
    equivalent.push(observation_with_expo(1, -3));
    equivalent.push(observation_with_expo(10, -4));
    equivalent.push(observation_with_expo(100, -5));

    assert!(!breaker.should_trip(&equivalent));
}

#[test]
fn windowed_change_delta_compares_current_to_lookback_window() {
    let breaker = WindowedChangeDelta {
        window_len: 2,
        lookback_windows: 1,
        max_relative_change_delta: dec("0.05"),
    };

    assert!(breaker.should_trip(&history([100, 101, 100, 110])));
    assert!(breaker.should_trip(&history([0, 0, 0, 1])));
}

#[test]
fn windowed_change_delta_accounts_for_price_exponent() {
    let breaker = WindowedChangeDelta {
        window_len: 2,
        lookback_windows: 1,
        max_relative_change_delta: dec("0.05"),
    };
    let mut equivalent = RingBuffer::new(4);
    equivalent.push(observation_with_expo(100, -2));
    equivalent.push(observation_with_expo(110, -2));
    equivalent.push(observation_with_expo(1, 0));
    equivalent.push(observation_with_expo(11, -1));

    assert!(!breaker.should_trip(&equivalent));
}

#[test]
fn set_adds_and_removes_breakers_by_id() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    let id = 0;
    set.add(id, breaker).unwrap();
    set.set_config(CircuitBreakerSetConfig {
        sample_interval_ns: Nanoseconds::zero(),
        history_len: 2,
    });
    set.try_accept_price(price(100), Nanoseconds::zero())
        .unwrap();

    assert_eq!(id, 0);
    assert_eq!(set.next_id(), 1);
    assert_eq!(set.accepted_history().get(0).unwrap().price, price(100));
    assert_eq!(set.observed_history().get(0).unwrap().price, price(100));

    set.remove(id).unwrap();

    assert!(set.breakers().is_empty());
}

#[test]
fn set_adds_breakers_with_explicit_monotonic_ids() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    let result = set.add(0, breaker.clone()).unwrap();
    assert!(matches!(
        result.events.as_slice(),
        [CircuitBreakerEvent::Added { breaker_id: 0, .. }]
    ));
    let result = set.add(1, breaker).unwrap();
    assert!(matches!(
        result.events.as_slice(),
        [CircuitBreakerEvent::Added { breaker_id: 1, .. }]
    ));
    assert_eq!(set.next_id(), 2);
    assert!(set.breakers().contains_key(&0));
    assert!(set.breakers().contains_key(&1));
}

#[test]
fn set_accepts_custom_rule_type() {
    struct AlwaysTrips;

    impl CircuitBreakerRule for AlwaysTrips {
        fn should_trip(&self, _: &RingBuffer<Observation>) -> bool {
            true
        }
    }

    let mut breakers = BTreeMap::new();
    breakers.insert(0, CircuitBreakerState::new(AlwaysTrips));
    let mut set = CircuitBreakerSet::try_from(UncheckedCircuitBreakerSet {
        sample_interval_ns: Nanoseconds::zero(),
        accepted_history: RingBuffer::new(1),
        observed_history: RingBuffer::new(1),
        next_id: 1,
        is_manually_tripped: false,
        breakers,
    })
    .unwrap();

    assert_blocked_by(
        set.try_accept_price(price(100), Nanoseconds::from_secs(1)),
        vec![0],
    );
}

#[test]
fn set_rejects_unexpected_breaker_id() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(
        set.add(1, breaker),
        Err(CircuitBreakerError::UnexpectedBreakerId {
            expected: 0,
            actual: 1
        })
    );
}

#[test]
fn set_rejects_invalid_price_without_recording_history() {
    let mut set = breaker_set(Nanoseconds::zero(), 1);

    assert_eq!(
        set.try_accept_price(price_with_conf(1, 1), Nanoseconds::from_secs(1)),
        Err(CircuitBreakerError::InvalidPrice)
    );
    assert!(set.accepted_history().is_empty());
    assert!(set.observed_history().is_empty());
}

fn unchecked_set_with_stale_next_id() -> UncheckedCircuitBreakerSet {
    let mut set = CircuitBreakerSet::empty();
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();
    let mut unchecked = UncheckedCircuitBreakerSet::from(set);
    unchecked.next_id = 0;
    unchecked
}

#[test]
fn set_rejects_parse_when_state_has_stale_next_id() {
    assert_eq!(
        CircuitBreakerSet::try_from(unchecked_set_with_stale_next_id()),
        Err(CircuitBreakerSetParseError::BreakerIdOutOfRange)
    );
}

#[cfg(feature = "borsh")]
#[test]
fn borsh_rejects_set_with_stale_next_id() {
    let bytes = borsh::to_vec(&unchecked_set_with_stale_next_id()).unwrap();

    assert!(borsh::from_slice::<CircuitBreakerSet>(&bytes).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn serde_rejects_set_with_stale_next_id() {
    let bytes = serde_json::to_vec(&unchecked_set_with_stale_next_id()).unwrap();

    assert!(serde_json::from_slice::<CircuitBreakerSet>(&bytes).is_err());
}

#[test]
fn set_rejects_parse_when_history_capacities_differ() {
    assert_eq!(
        CircuitBreakerSet::<CircuitBreaker>::try_from(UncheckedCircuitBreakerSet {
            sample_interval_ns: Nanoseconds::zero(),
            accepted_history: RingBuffer::new(1),
            observed_history: RingBuffer::new(2),
            next_id: 0,
            is_manually_tripped: false,
            breakers: BTreeMap::new(),
        }),
        Err(CircuitBreakerSetParseError::HistoryCapacityMismatch)
    );
}

#[cfg(feature = "serde")]
#[test]
fn serde_serializes_set_like_unchecked_representation() {
    let mut set = CircuitBreakerSet::empty();
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();
    let unchecked = UncheckedCircuitBreakerSet::from(set.clone());

    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        serde_json::to_value(&unchecked).unwrap()
    );
}

#[test]
fn set_rejects_add_when_next_id_is_exhausted() {
    let mut set = CircuitBreakerSet::try_from(UncheckedCircuitBreakerSet {
        sample_interval_ns: Nanoseconds::zero(),
        accepted_history: RingBuffer::new(0),
        observed_history: RingBuffer::new(0),
        next_id: u32::MAX,
        is_manually_tripped: false,
        breakers: BTreeMap::new(),
    })
    .unwrap();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(
        set.add(u32::MAX, breaker),
        Err(CircuitBreakerError::TooManyBreakers)
    );
}

#[test]
fn set_prioritizes_unexpected_breaker_id_over_exhaustion() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(
        set.add(u32::MAX, breaker),
        Err(CircuitBreakerError::UnexpectedBreakerId {
            expected: 0,
            actual: u32::MAX
        })
    );
}

#[test]
fn future_armed_breaker_records_history_without_tripping() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.01"),
        }),
    )
    .unwrap();
    set.rearm(id, Nanoseconds::from_secs(10), AcceptedHistorySource::Empty)
        .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    set.try_accept_price(price(200), Nanoseconds::from_secs(2))
        .unwrap();

    assert_eq!(set.accepted_history().len(), 2);
    assert_eq!(set.observed_history().len(), 2);
    assert!(matches!(
        set.breakers().get(&0).unwrap().status,
        CircuitBreakerStatus::ArmedAfter { .. }
    ));
}

#[test]
fn set_returns_tripped_for_new_and_existing_trips() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    let acceptance = assert_blocked_by(
        set.try_accept_price(price(111), Nanoseconds::from_secs(2)),
        vec![id],
    );
    assert_eq!(acceptance.events.len(), 1);
    assert!(matches!(
        acceptance.events[0],
        CircuitBreakerEvent::Tripped {
            breaker_id,
            is_enforced: true,
            ..
        } if breaker_id == id
    ));
    let acceptance = assert_blocked_by(
        set.try_accept_price(price(111), Nanoseconds::from_secs(3)),
        vec![id],
    );
    assert!(acceptance.events.is_empty());
    assert_eq!(set.accepted_history().get(0).unwrap().price, price(100));
    assert_eq!(
        set.observed_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![111, 111]
    );
}

#[test]
fn set_returns_first_new_blocking_breaker_id() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let first_id = 0;
    set.add(
        first_id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();
    let second_id = 1;
    set.add(
        second_id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.20"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();

    assert_blocked_by(
        set.try_accept_price(price(150), Nanoseconds::from_secs(2)),
        vec![first_id],
    );
}

#[test]
fn too_soon_sample_can_trip_without_being_persisted() {
    let mut set = breaker_set(Nanoseconds::from_secs(10), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    assert_blocked_by(
        set.try_accept_price(price(200), Nanoseconds::from_secs(2)),
        vec![id],
    );

    assert_eq!(set.accepted_history().len(), 1);
    assert_eq!(set.accepted_history().get(0).unwrap().price, price(100));
    assert_eq!(set.observed_history().len(), 1);
    assert_eq!(set.observed_history().get(0).unwrap().price, price(100));
    assert_eq!(
        set.accepted_history().get(0).unwrap().observed_at_ns,
        Nanoseconds::from_secs(1)
    );
    let breaker = set.breakers().get(&0).unwrap();
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::Tripped {
            price_update,
            ..
        } if price_update.price == price(200) && price_update.observed_at_ns == Nanoseconds::from_secs(2)
    ));
}

#[test]
fn cumulative_too_soon_drift_trips_against_persisted_baseline() {
    let mut set = breaker_set(Nanoseconds::from_secs(10), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    set.try_accept_price(price(105), Nanoseconds::from_secs(2))
        .unwrap();
    set.try_accept_price(price(109), Nanoseconds::from_secs(3))
        .unwrap();
    assert_blocked_by(
        set.try_accept_price(price(111), Nanoseconds::from_secs(4)),
        vec![id],
    );

    assert_eq!(set.accepted_history().len(), 1);
    assert_eq!(set.accepted_history().get(0).unwrap().price, price(100));
    assert_eq!(set.observed_history().len(), 1);
    assert_eq!(set.observed_history().get(0).unwrap().price, price(100));
}

#[test]
fn blocked_observed_history_respects_sample_interval() {
    let mut set = breaker_set(Nanoseconds::from_secs(10), 3);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    set.set_manual_trip(true, actor_id(), None);
    assert_manually_blocked(set.try_accept_price(price(101), Nanoseconds::from_secs(2)));
    assert_eq!(set.observed_history().len(), 1);
    assert_manually_blocked(set.try_accept_price(price(102), Nanoseconds::from_secs(11)));

    assert_eq!(
        set.observed_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![100, 102]
    );
}

#[test]
fn unenforced_and_tripped_breakers_still_record_history() {
    let mut set = breaker_set(Nanoseconds::zero(), 3);
    let unenforced_id = 0;
    set.add(
        unenforced_id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.01"),
        }),
    )
    .unwrap();
    let tripped_id = 1;
    set.add(
        tripped_id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.01"),
        }),
    )
    .unwrap();

    set.set_enforced(unenforced_id, false).unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();

    let acceptance = assert_blocked_by(
        set.try_accept_price(price(200), Nanoseconds::from_secs(2)),
        vec![tripped_id],
    );
    assert_eq!(acceptance.events.len(), 2);
    assert!(matches!(
        acceptance.events[0],
        CircuitBreakerEvent::Tripped {
            breaker_id,
            is_enforced: false,
            ..
        } if breaker_id == unenforced_id
    ));
    assert!(matches!(
        acceptance.events[1],
        CircuitBreakerEvent::Tripped {
            breaker_id,
            is_enforced: true,
            ..
        } if breaker_id == tripped_id
    ));

    assert_eq!(set.accepted_history().len(), 1);
    assert_eq!(set.observed_history().len(), 2);
    assert!(!set.breakers().get(&0).unwrap().is_enforced);
    assert!(matches!(
        set.breakers().get(&1).unwrap().status,
        CircuitBreakerStatus::Tripped { .. }
    ));
}

#[test]
fn unenforced_breaker_can_trip_without_blocking_until_enforced() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.set_enforced(id, false).unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    let acceptance = set
        .try_accept_price(price(120), Nanoseconds::from_secs(2))
        .unwrap();
    assert!(acceptance.value.is_ok());
    assert_eq!(acceptance.events.len(), 1);
    assert!(matches!(
        acceptance.events[0],
        CircuitBreakerEvent::Tripped {
            breaker_id,
            is_enforced: false,
            ..
        } if breaker_id == id
    ));

    let breaker = set.breakers().get(&0).unwrap();
    assert!(!breaker.is_enforced);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::Tripped { .. }
    ));
    assert!(!set.is_blocking());

    set.set_enforced(id, true).unwrap();

    assert!(set.is_blocking());
    assert_blocked_by(
        set.try_accept_price(price(130), Nanoseconds::from_secs(3)),
        vec![id],
    );
}

#[test]
fn set_enforced_returns_empty_outcome_when_value_is_unchanged() {
    let mut set = breaker_set(Nanoseconds::zero(), 1);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    let unchanged = set.set_enforced(id, true).unwrap();

    assert!(unchanged.events.is_empty());
    assert!(set.breakers().get(&id).unwrap().is_enforced);

    let changed = set.set_enforced(id, false).unwrap();

    assert_eq!(changed.events.len(), 1);
    assert!(matches!(
        changed.events[0],
        CircuitBreakerEvent::EnforcementSet {
            breaker_id,
            is_enforced: false,
        } if breaker_id == id
    ));
}

#[test]
fn armed_after_zero_clears_tripped_status_without_enforcing_breaker() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    assert_blocked_by(
        set.try_accept_price(price(120), Nanoseconds::from_secs(2)),
        vec![id],
    );
    set.set_enforced(id, false).unwrap();
    set.rearm(id, Nanoseconds::zero(), AcceptedHistorySource::Empty)
        .unwrap();

    let breaker = set.breakers().get(&0).unwrap();
    assert!(!breaker.is_enforced);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::ArmedAfter {
            timestamp_ns
        } if timestamp_ns == Nanoseconds::zero()
    ));
    assert!(!set.is_blocking());
}

#[test]
fn manual_trip_override_blocks_set_without_tripping_breaker() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    set.set_manual_trip(true, actor_id(), None);

    assert!(set.is_blocking());
    assert_manually_blocked(set.try_accept_price(price(100), Nanoseconds::from_secs(5)));
    assert!(set.accepted_history().is_empty());
    assert_eq!(set.observed_history().get(0).unwrap().price, price(100));
}

#[test]
fn accepted_history_can_be_cleared_or_seeded_from_observed_history() {
    let mut set = breaker_set(Nanoseconds::zero(), 3);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("10"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    set.set_manual_trip(true, actor_id(), None);
    assert_manually_blocked(set.try_accept_price(price(200), Nanoseconds::from_secs(2)));

    assert_eq!(set.accepted_history().get(0).unwrap().price, price(100));
    assert_eq!(
        set.observed_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![100, 200]
    );

    set.rearm(0, Nanoseconds::zero(), AcceptedHistorySource::Empty)
        .unwrap();
    assert!(set.accepted_history().is_empty());

    set.rearm(0, Nanoseconds::zero(), AcceptedHistorySource::Observed)
        .unwrap();
    assert_eq!(
        set.accepted_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![100, 200]
    );
}

#[test]
fn set_config_resizes_history_in_place() {
    let mut set = breaker_set(Nanoseconds::zero(), 3);

    set.try_accept_price(price(100), Nanoseconds::from_secs(1))
        .unwrap();
    set.try_accept_price(price(200), Nanoseconds::from_secs(2))
        .unwrap();
    set.try_accept_price(price(300), Nanoseconds::from_secs(3))
        .unwrap();

    set.set_config(CircuitBreakerSetConfig {
        sample_interval_ns: Nanoseconds::from_secs(10),
        history_len: 2,
    });

    assert_eq!(set.sample_interval_ns(), Nanoseconds::from_secs(10));
    assert_eq!(
        set.accepted_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![200, 300]
    );
    assert_eq!(
        set.observed_history()
            .iter()
            .map(|observation| observation.price.price)
            .collect::<Vec<_>>(),
        vec![200, 300]
    );
}

#[test]
fn rule_trip_records_causal_price_update() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.01"),
        }),
    )
    .unwrap();

    set.try_accept_price(price(100), Nanoseconds::from_secs(4))
        .unwrap();
    let acceptance = assert_blocked_by(
        set.try_accept_price(price(200), Nanoseconds::from_secs(5)),
        vec![id],
    );
    assert_eq!(acceptance.events.len(), 1);

    assert!(matches!(
        set.breakers().get(&0).unwrap().status,
        CircuitBreakerStatus::Tripped {
            tripped_at_ns,
            price_update,
        } if tripped_at_ns == Nanoseconds::from_secs(5)
            && price_update.price == price(200)
            && price_update.observed_at_ns == Nanoseconds::from_secs(5)
    ));
}

fn production_breaker_set(history_len: u32) -> CircuitBreakerSet {
    let mut set = breaker_set(Nanoseconds::zero(), history_len);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();
    set.add(
        1,
        CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 3,
            min_relative_step_change: dec("0.01"),
        }),
    )
    .unwrap();
    set.add(
        2,
        CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
            window_len: 2,
            lookback_windows: 3,
            max_relative_change_delta: dec("0.15"),
        }),
    )
    .unwrap();
    set
}

fn xlm_normal_prices() -> Vec<i64> {
    vec![
        1609, 1624, 1651, 1587, 1612, 1635, 1598, 1642,
        1661, 1628, 1655, 1584, 1601, 1638, 1619, 1653,
        1627, 1592, 1615, 1631,
    ]
}

fn stable_normal_prices() -> Vec<i64> {
    vec![
        10000, 10002, 9998, 10001, 9999, 10003, 9997, 10000,
        10001, 9998, 10002, 9999, 10001, 10000, 9999, 10002,
        9998, 10001, 10000, 10002,
    ]
}

#[rstest]
#[case::xlm(xlm_normal_prices())]
#[case::stable(stable_normal_prices())]
fn production_config_accepts_normal_prices(#[case] prices: Vec<i64>) {
    let mut set = production_breaker_set(8);
    for (i, price_value) in prices.iter().enumerate() {
        let result = set.try_accept_price(
            price(*price_value),
            Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().value.is_ok());
    }
}

fn blend_ustry_prices() -> Vec<i64> {
    vec![10574, 10574, 1067372830, 1067372830]
}

fn grass_real_prices() -> Vec<i64> {
    vec![
        440133, 462253, 429890, 435210, 415743, 385436, 383878, 368338, 368879, 366859,
        366570, 365221, 360165, 358117, 359797, 362481, 368742, 374651, 373106, 379641,
        363731, 372978, 372647, 360959, 352902, 348971, 339410, 339482, 341627, 345444,
        348634, 356240, 350571, 349272, 339563, 331869, 340445, 338333, 327583, 329394,
        318741, 327444, 324645, 327746, 328008, 329366, 328617, 330883, 325752, 325926,
        320168, 324288, 328065, 326627, 333977, 327487, 329860, 330131, 330232, 342978,
        344488, 357243, 351515, 334700, 341617, 337901, 346614, 339060, 356017, 352919,
        362861, 346541, 349552, 344890, 349830, 366431, 362651, 369198, 370224, 353182,
        363019, 367602, 361459, 363678, 366458, 363910, 368817, 361983, 360381, 373445,
        374647, 382256, 393647, 379494, 374771, 387318, 383553, 379056, 377827, 377512,
        378667, 386049, 379317, 370275, 363544, 362811, 357520, 367252, 367244, 358386,
        346554, 343958, 337224, 343228, 344858, 349943, 347051, 346437, 334993, 328010,
        329596, 326459, 327727, 330529, 338650, 337498, 336740, 334862, 329506, 336087,
        311847, 316251, 314139, 316279, 306281, 298804, 301298, 304293, 304604, 299734,
        304342, 305594, 302027, 301037, 302689, 299103, 295597, 295408, 294308, 292579,
        300290, 300372, 302460, 298805, 298090, 305194, 298532, 297038, 303092, 298869,
        310369, 323379, 319568, 332360, 338860, 338202, 360602, 382202, 423528, 430555,
        445170, 455489, 438011, 421636, 407938, 400542, 410839, 415488, 438073, 499318,
        536482,
    ]
}

fn btc_recent_prices() -> Vec<i64> {
    vec![
        7826061, 7774745, 7763147, 7824679, 7804733, 7764062, 7744479, 7765999, 7755243, 7768105,
        7736937, 7733426, 7761914, 7742196, 7810400, 7811088, 7805075, 7824093, 7864512, 7909632,
        7765342, 7783913, 7677256, 7682948, 7736129, 7680477, 7687194, 7618261, 7604086, 7634013,
        7634522, 7696146, 7702371, 7757959, 7585812, 7546890, 7577488, 7590970, 7608514, 7600583,
        7645576, 7639195, 7628657, 7708522, 7709621, 7743364, 7843482, 7842358, 7817207, 7843793,
        7819406, 7812846, 7846163, 7844753, 7866602, 7818840, 7835069, 7865190, 7864851, 7877293,
        7854280, 8025407, 7968931, 7879453, 7996147, 8005207, 7982440, 8086232, 8083201, 8098088,
        8153144, 8161009, 8092509, 8158474, 8132911, 8249620, 8168052, 8146738, 8142499, 8086642,
        8149772, 8085804, 7989548, 8008682, 8002204, 7956588, 7966644, 8021869, 8010466, 8009551,
        8018906, 8038129, 8021738, 8035036, 8051325, 8089473, 8067803, 8078486, 8071338, 8082327,
        8140510, 8141102, 8214565, 8069497, 8071023, 8114989, 8138384, 8193913, 8172520, 8102626,
        8086223, 8074579, 8033659, 8078826, 8048088, 8119570, 8097626, 8048493, 7882754, 7956336,
        7927780, 7897318, 7976637, 7926405, 8129332, 8138690, 8105198, 8105527, 8079651, 8060609,
        7914059, 7912117, 7907154, 7905781, 7833577, 7805193, 7820291, 7821681, 7813500, 7797988,
        7813611, 7836861, 7801109, 7835707, 7743249, 7690239, 7701212, 7724708, 7637969, 7682681,
        7695221, 7671184, 7716306, 7666924, 7643507, 7674963, 7675295, 7666010, 7719776, 7731701,
        7735858, 7761283, 7745994, 7799892, 7780170, 7712921, 7718016, 7762965, 7754633, 7768964,
        7732499, 7732151, 7671698, 7578462, 7548252, 7554772, 7450144, 7472662, 7542942, 7585936,
        7653165,
    ]
}

fn btc_oct_2025_prices() -> Vec<i64> {
    vec![
        10365414, 10339608, 10298231, 10263483, 10224334, 10229736, 10228986, 10175900, 10189291, 10170444,
        10190311, 10199993, 10226298, 10229013, 10200972, 10164512, 10185541, 10190023, 10164614, 10222399,
        10280615, 10377193, 10359001, 10476327, 10449278, 10470967, 10579536, 10606326, 10605134, 10638941,
        10632274, 10621753, 10643308, 10481708, 10576180, 10598096, 10536564, 10595149, 10640758, 10642115,
        10536800, 10481713, 10497928, 10525974, 10436567, 10344340, 10333389, 10313212, 10266741, 10311221,
        10328924, 10334208, 10333903, 10312780, 10449238, 10495450, 10502275, 10214595, 10178823, 10126093,
    ]
}

#[test]
fn production_config_blocks_blend_exploit_stepwise_change() {
    let mut set = production_breaker_set(8);
    let prices = blend_ustry_prices();

    for (i, price_value) in prices.iter().take(2).enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        assert!(result.value.is_ok());
    }

    let result = set.try_accept_price(price(prices[2]), Nanoseconds::from_secs(3));
    assert!(result.is_ok());
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
    assert!(
        matches!(
            acceptance.value,
            Err(PriceBlockedReason::BreakerTripped { ref blocking_breaker_ids })
            if blocking_breaker_ids.contains(&0)
        ),
        "Blend exploit should be blocked by StepwiseChange, got {:?}",
        acceptance.value
    );

    assert_eq!(
        set.accepted_history().as_slice().last().unwrap().price.price,
        prices[1]
    );

    let result = set.try_accept_price(price(prices[3]), Nanoseconds::from_secs(4));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
}

#[test]
fn production_config_blocks_sustained_pump_monotonic_run() {
    let mut set = production_breaker_set(8);
    let pump_prices = vec![100, 105, 110, 116, 122, 128];

    for (i, price_value) in pump_prices.iter().take(3).enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        assert!(result.value.is_ok());
    }

    let result = set.try_accept_price(price(pump_prices[3]), Nanoseconds::from_secs(4));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
    assert!(
        matches!(
            acceptance.value,
            Err(PriceBlockedReason::BreakerTripped { ref blocking_breaker_ids })
            if blocking_breaker_ids.contains(&1)
        ),
        "Sustained pump should be blocked by MonotonicRun, got {:?}",
        acceptance.value
    );
}

#[test]
fn windowed_change_delta_blocks_statistical_outlier() {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
            window_len: 2,
            lookback_windows: 3,
            max_relative_change_delta: dec("0.15"),
        }),
    )
    .unwrap();

    let stable_history = vec![10000, 10001, 9999, 10002, 10000, 9998, 10001, 10000];
    for (i, price_value) in stable_history.iter().enumerate() {
        set.try_accept_price(
            price(*price_value),
            Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
        )
        .unwrap();
    }

    let result = set.try_accept_price(price(50000), Nanoseconds::from_secs(9));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
    assert!(
        matches!(
            acceptance.value,
            Err(PriceBlockedReason::BreakerTripped { ref blocking_breaker_ids })
            if blocking_breaker_ids.contains(&0)
        ),
        "Statistical outlier should be blocked by WindowedChangeDelta, got {:?}",
        acceptance.value
    );
}

#[test]
fn blend_exploit_cumulative_defense_all_breakers_together() {
    let mut set = production_breaker_set(16);
    let normal_prices = vec![10574, 10574, 10583, 10568, 10580];
    for (i, price_value) in normal_prices.iter().enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        assert!(result.value.is_ok());
    }

    let manipulated_price = 1067372830;
    let result = set.try_accept_price(price(manipulated_price), Nanoseconds::from_secs(6));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());

    match acceptance.value {
        Err(PriceBlockedReason::BreakerTripped { blocking_breaker_ids }) => {
            assert!(blocking_breaker_ids.contains(&0));
        }
        other => panic!("Expected BreakerTripped, got {:?}", other),
    }

    let last_accepted = set.accepted_history().as_slice().last().unwrap().price.price;
    assert_eq!(last_accepted, 10580);

    let result = set.try_accept_price(price(manipulated_price), Nanoseconds::from_secs(7));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
    assert!(acceptance.events.is_empty());
}

#[test]
fn real_grass_data_passes_with_relaxed_stepwise_change() {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.15"),
        }),
    )
    .unwrap();

    let prices = grass_real_prices();
    let mut blocked_count = 0;
    for (i, price_value) in prices.iter().enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        if result.value.is_err() {
            blocked_count += 1;
        }
    }

    assert_eq!(
        blocked_count, 0,
        "All {} real GRASS price points should pass with 15% StepwiseChange, but {} were blocked",
        prices.len(), blocked_count
    );
}

#[test]
fn real_grass_data_13_98_percent_jump_trips_strict_stepwise_change() {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    let prices = grass_real_prices();
    let mut blocked_at = None;
    for (i, price_value) in prices.iter().enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        if result.value.is_err() && blocked_at.is_none() {
            blocked_at = Some(i);
        }
    }

    assert_eq!(
        blocked_at,
        Some(168),
        "First blockage should be at index 168 with 10% StepwiseChange"
    );
}

#[test]
fn real_grass_data_with_simulated_100x_manipulation_blocked() {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.15"),
        }),
    )
    .unwrap();

    let prices = grass_real_prices();
    for (i, price_value) in prices.iter().enumerate() {
        set.try_accept_price(
            price(*price_value),
            Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
        )
        .unwrap();
    }

    let last_real_price = prices.last().copied().unwrap();
    let manipulated_price = last_real_price * 100;

    let result = set.try_accept_price(
        price(manipulated_price),
        Nanoseconds::from_secs(u64::try_from(prices.len() + 1).unwrap()),
    );
    let acceptance = result.unwrap();
    assert!(
        acceptance.value.is_err(),
        "100x manipulation on real GRASS data should be blocked, got {:?}",
        acceptance.value
    );
}

#[rstest]
#[case::btc_recent(btc_recent_prices(), "recent BTC")]
#[case::btc_oct_2025(btc_oct_2025_prices(), "Oct 2025 BTC")]
fn real_btc_data_passes_with_10_percent_stepwise(
    #[case] prices: Vec<i64>,
    #[case] label: &str,
) {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    let mut blocked_count = 0;
    for (i, price_value) in prices.iter().enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        if result.value.is_err() {
            blocked_count += 1;
        }
    }

    assert_eq!(
        blocked_count, 0,
        "All {} {} price points should pass with 10% StepwiseChange",
        prices.len(), label
    );
}

#[test]
fn real_btc_with_blend_exploit_blocked() {
    let mut set = breaker_set(Nanoseconds::zero(), 16);
    set.add(
        0,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    let prices = btc_recent_prices();
    for (i, price_value) in prices.iter().enumerate() {
        set.try_accept_price(
            price(*price_value),
            Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
        )
        .unwrap();
    }

    let last_real_price = prices.last().copied().unwrap();
    let manipulated_price = last_real_price * 100;

    let result = set.try_accept_price(
        price(manipulated_price),
        Nanoseconds::from_secs(u64::try_from(prices.len() + 1).unwrap()),
    );
    let acceptance = result.unwrap();
    assert!(
        acceptance.value.is_err(),
        "100x manipulation on real BTC data should be blocked, got {:?}",
        acceptance.value
    );
}
