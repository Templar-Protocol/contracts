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
        1609, 1624, 1651, 1587, 1612, 1635, 1598, 1642, 1661, 1628, 1655, 1584, 1601, 1638, 1619,
        1653, 1627, 1592, 1615, 1631,
    ]
}

fn stable_normal_prices() -> Vec<i64> {
    vec![
        10000, 10002, 9998, 10001, 9999, 10003, 9997, 10000, 10001, 9998, 10002, 9999, 10001,
        10000, 9999, 10002, 9998, 10001, 10000, 10002,
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
    vec![10574, 10574, 1_067_372_830, 1_067_372_830]
}

fn grass_real_prices() -> Vec<i64> {
    vec![
        440_133, 462_253, 429_890, 435_210, 415_743, 385_436, 383_878, 368_338, 368_879, 366_859,
        366_570, 365_221, 360_165, 358_117, 359_797, 362_481, 368_742, 374_651, 373_106, 379_641,
        363_731, 372_978, 372_647, 360_959, 352_902, 348_971, 339_410, 339_482, 341_627, 345_444,
        348_634, 356_240, 350_571, 349_272, 339_563, 331_869, 340_445, 338_333, 327_583, 329_394,
        318_741, 327_444, 324_645, 327_746, 328_008, 329_366, 328_617, 330_883, 325_752, 325_926,
        320_168, 324_288, 328_065, 326_627, 333_977, 327_487, 329_860, 330_131, 330_232, 342_978,
        344_488, 357_243, 351_515, 334_700, 341_617, 337_901, 346_614, 339_060, 356_017, 352_919,
        362_861, 346_541, 349_552, 344_890, 349_830, 366_431, 362_651, 369_198, 370_224, 353_182,
        363_019, 367_602, 361_459, 363_678, 366_458, 363_910, 368_817, 361_983, 360_381, 373_445,
        374_647, 382_256, 393_647, 379_494, 374_771, 387_318, 383_553, 379_056, 377_827, 377_512,
        378_667, 386_049, 379_317, 370_275, 363_544, 362_811, 357_520, 367_252, 367_244, 358_386,
        346_554, 343_958, 337_224, 343_228, 344_858, 349_943, 347_051, 346_437, 334_993, 328_010,
        329_596, 326_459, 327_727, 330_529, 338_650, 337_498, 336_740, 334_862, 329_506, 336_087,
        311_847, 316_251, 314_139, 316_279, 306_281, 298_804, 301_298, 304_293, 304_604, 299_734,
        304_342, 305_594, 302_027, 301_037, 302_689, 299_103, 295_597, 295_408, 294_308, 292_579,
        300_290, 300_372, 302_460, 298_805, 298_090, 305_194, 298_532, 297_038, 303_092, 298_869,
        310_369, 323_379, 319_568, 332_360, 338_860, 338_202, 360_602, 382_202, 423_528, 430_555,
        445_170, 455_489, 438_011, 421_636, 407_938, 400_542, 410_839, 415_488, 438_073, 499_318,
        536_482,
    ]
}

fn btc_recent_prices() -> Vec<i64> {
    vec![
        7_826_061, 7_774_745, 7_763_147, 7_824_679, 7_804_733, 7_764_062, 7_744_479, 7_765_999,
        7_755_243, 7_768_105, 7_736_937, 7_733_426, 7_761_914, 7_742_196, 7_810_400, 7_811_088,
        7_805_075, 7_824_093, 7_864_512, 7_909_632, 7_765_342, 7_783_913, 7_677_256, 7_682_948,
        7_736_129, 7_680_477, 7_687_194, 7_618_261, 7_604_086, 7_634_013, 7_634_522, 7_696_146,
        7_702_371, 7_757_959, 7_585_812, 7_546_890, 7_577_488, 7_590_970, 7_608_514, 7_600_583,
        7_645_576, 7_639_195, 7_628_657, 7_708_522, 7_709_621, 7_743_364, 7_843_482, 7_842_358,
        7_817_207, 7_843_793, 7_819_406, 7_812_846, 7_846_163, 7_844_753, 7_866_602, 7_818_840,
        7_835_069, 7_865_190, 7_864_851, 7_877_293, 7_854_280, 8_025_407, 7_968_931, 7_879_453,
        7_996_147, 8_005_207, 7_982_440, 8_086_232, 8_083_201, 8_098_088, 8_153_144, 8_161_009,
        8_092_509, 8_158_474, 8_132_911, 8_249_620, 8_168_052, 8_146_738, 8_142_499, 8_086_642,
        8_149_772, 8_085_804, 7_989_548, 8_008_682, 8_002_204, 7_956_588, 7_966_644, 8_021_869,
        8_010_466, 8_009_551, 8_018_906, 8_038_129, 8_021_738, 8_035_036, 8_051_325, 8_089_473,
        8_067_803, 8_078_486, 8_071_338, 8_082_327, 8_140_510, 8_141_102, 8_214_565, 8_069_497,
        8_071_023, 8_114_989, 8_138_384, 8_193_913, 8_172_520, 8_102_626, 8_086_223, 8_074_579,
        8_033_659, 8_078_826, 8_048_088, 8_119_570, 8_097_626, 8_048_493, 7_882_754, 7_956_336,
        7_927_780, 7_897_318, 7_976_637, 7_926_405, 8_129_332, 8_138_690, 8_105_198, 8_105_527,
        8_079_651, 8_060_609, 7_914_059, 7_912_117, 7_907_154, 7_905_781, 7_833_577, 7_805_193,
        7_820_291, 7_821_681, 7_813_500, 7_797_988, 7_813_611, 7_836_861, 7_801_109, 7_835_707,
        7_743_249, 7_690_239, 7_701_212, 7_724_708, 7_637_969, 7_682_681, 7_695_221, 7_671_184,
        7_716_306, 7_666_924, 7_643_507, 7_674_963, 7_675_295, 7_666_010, 7_719_776, 7_731_701,
        7_735_858, 7_761_283, 7_745_994, 7_799_892, 7_780_170, 7_712_921, 7_718_016, 7_762_965,
        7_754_633, 7_768_964, 7_732_499, 7_732_151, 7_671_698, 7_578_462, 7_548_252, 7_554_772,
        7_450_144, 7_472_662, 7_542_942, 7_585_936, 7_653_165,
    ]
}

fn btc_oct_2025_prices() -> Vec<i64> {
    vec![
        10_365_414, 10_339_608, 10_298_231, 10_263_483, 10_224_334, 10_229_736, 10_228_986,
        10_175_900, 10_189_291, 10_170_444, 10_190_311, 10_199_993, 10_226_298, 10_229_013,
        10_200_972, 10_164_512, 10_185_541, 10_190_023, 10_164_614, 10_222_399, 10_280_615,
        10_377_193, 10_359_001, 10_476_327, 10_449_278, 10_470_967, 10_579_536, 10_606_326,
        10_605_134, 10_638_941, 10_632_274, 10_621_753, 10_643_308, 10_481_708, 10_576_180,
        10_598_096, 10_536_564, 10_595_149, 10_640_758, 10_642_115, 10_536_800, 10_481_713,
        10_497_928, 10_525_974, 10_436_567, 10_344_340, 10_333_389, 10_313_212, 10_266_741,
        10_311_221, 10_328_924, 10_334_208, 10_333_903, 10_312_780, 10_449_238, 10_495_450,
        10_502_275, 10_214_595, 10_178_823, 10_126_093,
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
        set.accepted_history().last().unwrap().price.price,
        prices[1]
    );

    let result = set.try_accept_price(price(prices[3]), Nanoseconds::from_secs(4));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());
}

#[test]
fn production_config_blocks_sustained_pump_monotonic_run() {
    let mut set = production_breaker_set(8);
    let pump_prices = [100, 105, 110, 116, 122, 128];

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

    let stable_history = [10000, 10001, 9999, 10002, 10000, 9998, 10001, 10000];
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
    let normal_prices = [10574, 10574, 10583, 10568, 10580];
    for (i, price_value) in normal_prices.iter().enumerate() {
        let result = set
            .try_accept_price(
                price(*price_value),
                Nanoseconds::from_secs(u64::try_from(i + 1).unwrap()),
            )
            .unwrap();
        assert!(result.value.is_ok());
    }

    let manipulated_price = 1_067_372_830;
    let result = set.try_accept_price(price(manipulated_price), Nanoseconds::from_secs(6));
    let acceptance = result.unwrap();
    assert!(acceptance.value.is_err());

    match acceptance.value {
        Err(PriceBlockedReason::BreakerTripped {
            blocking_breaker_ids,
        }) => {
            assert!(blocking_breaker_ids.contains(&0));
        }
        other => panic!("Expected BreakerTripped, got {:?}", other),
    }

    let last_accepted = set.accepted_history().last().unwrap().price.price;
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
        blocked_count,
        0,
        "All {} real GRASS price points should pass with 15% StepwiseChange, but {} were blocked",
        prices.len(),
        blocked_count
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
fn real_btc_data_passes_with_10_percent_stepwise(#[case] prices: Vec<i64>, #[case] label: &str) {
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
        blocked_count,
        0,
        "All {} {} price points should pass with 10% StepwiseChange",
        prices.len(),
        label
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
