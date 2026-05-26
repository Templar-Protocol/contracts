use core::str::FromStr;

use alloc::{collections::BTreeMap, vec, vec::Vec};
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
