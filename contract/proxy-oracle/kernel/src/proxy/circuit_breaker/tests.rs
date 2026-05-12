use core::str::FromStr;

use alloc::{vec, vec::Vec};
#[cfg(all(feature = "borsh", feature = "serde"))]
use std::eprintln;
use templar_primitives::{Decimal, Nanoseconds};

use crate::Price;

use super::*;

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn price(value: i64) -> Price {
    price_with_expo(value, 0)
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
        set.history.push(Observation {
            price: price(i64::from(100 + i)),
            observed_at_ns: Nanoseconds::from_secs(u64::from(i)),
        });
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
    set.history.push(observation(100));

    assert_eq!(id, 0);
    assert_eq!(set.next_id, 1);
    assert_eq!(set.history.as_slice(), &[observation(100)]);

    set.remove(id).unwrap();

    assert!(set.breakers.is_empty());
}

#[test]
fn set_adds_breakers_with_explicit_monotonic_ids() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(set.add(0, breaker.clone()), Ok(()));
    assert_eq!(set.add(1, breaker), Ok(()));
    assert_eq!(set.next_id, 2);
    assert!(set.breakers.contains_key(&0));
    assert!(set.breakers.contains_key(&1));
}

#[test]
fn set_rejects_unexpected_breaker_id() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(
        set.add(1, breaker),
        Err(Error::UnexpectedBreakerId {
            expected: 0,
            actual: 1
        })
    );
}

#[test]
fn set_rejects_add_when_next_id_is_exhausted() {
    let mut set = CircuitBreakerSet::empty();
    set.next_id = u32::MAX;
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });

    assert_eq!(set.add(u32::MAX, breaker), Err(Error::TooManyBreakers));
}

#[test]
fn muted_breaker_records_history_without_tripping() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.01"),
        }),
    )
    .unwrap();
    set.get_mut(id).unwrap().status = CircuitBreakerStatus::Muted {
        until_ns: Nanoseconds::from_secs(10),
    };

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    set.evaluate(price(200), Nanoseconds::from_secs(2)).unwrap();

    assert_eq!(set.history.len(), 2);
    assert!(matches!(
        set.breakers.get(&0).unwrap().status,
        CircuitBreakerStatus::Muted { .. }
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

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(111), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );
    assert_eq!(
        set.evaluate(price(111), Nanoseconds::from_secs(3)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );
}

#[test]
fn set_returns_all_blocking_breaker_ids() {
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

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();

    assert_eq!(
        set.evaluate(price(150), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_ids: vec![first_id, second_id]
        })
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

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );

    assert_eq!(set.history.len(), 1);
    assert_eq!(set.history.as_slice()[0].price, price(100));
    assert_eq!(
        set.history.as_slice()[0].observed_at_ns,
        Nanoseconds::from_secs(1)
    );
    let breaker = set.breakers.get(&0).unwrap();
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::Tripped {
            price_update,
            ..
        } if price_update.price == price(200) && price_update.observed_at_ns == Nanoseconds::from_secs(2)
    ));
}

#[test]
fn disabled_and_tripped_breakers_still_record_history() {
    let mut set = breaker_set(Nanoseconds::zero(), 3);
    let disabled_id = 0;
    set.add(
        disabled_id,
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

    set.get_mut(disabled_id).unwrap().is_enabled = false;

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();

    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_ids: vec![tripped_id],
        })
    );

    assert_eq!(set.history.len(), 2);
    assert!(!set.breakers.get(&0).unwrap().is_enabled);
    assert!(matches!(
        set.breakers.get(&1).unwrap().status,
        CircuitBreakerStatus::Tripped { .. }
    ));
}

#[test]
fn disabled_breaker_can_trip_without_blocking_until_enabled() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.get_mut(id).unwrap().is_enabled = false;

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    set.evaluate(price(120), Nanoseconds::from_secs(2)).unwrap();

    let breaker = set.breakers.get(&0).unwrap();
    assert!(!breaker.is_enabled);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::Tripped { .. }
    ));
    assert!(!set.is_blocking());

    set.get_mut(id).unwrap().is_enabled = true;

    assert!(set.is_blocking());
    assert_eq!(
        set.evaluate(price(130), Nanoseconds::from_secs(3)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );
}

#[test]
fn arm_clears_tripped_status_without_enabling_breaker() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = 0;
    set.add(
        id,
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: dec("0.10"),
        }),
    )
    .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(120), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );
    {
        let breaker = set.get_mut(id).unwrap();
        breaker.is_enabled = false;
        breaker.status = CircuitBreakerStatus::Armed;
    }

    let breaker = set.breakers.get(&0).unwrap();
    assert!(!breaker.is_enabled);
    assert!(matches!(breaker.status, CircuitBreakerStatus::Armed));
    assert!(!set.is_blocking());
}

#[test]
fn manual_trip_override_blocks_set_without_tripping_breaker() {
    let mut set = CircuitBreakerSet::empty();
    set.set_manual_trip(true);

    assert!(set.is_blocking());
    assert_eq!(
        set.evaluate(price(100), Nanoseconds::from_secs(5)),
        Err(Error::ManuallyTripped)
    );
}

#[test]
fn set_config_resizes_history_in_place() {
    let mut set = breaker_set(Nanoseconds::zero(), 3);

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    set.evaluate(price(200), Nanoseconds::from_secs(2)).unwrap();
    set.evaluate(price(300), Nanoseconds::from_secs(3)).unwrap();

    set.set_config(CircuitBreakerSetConfig {
        sample_interval_ns: Nanoseconds::from_secs(10),
        history_len: 2,
    });

    assert_eq!(set.sample_interval_ns, Nanoseconds::from_secs(10));
    assert_eq!(
        set.history
            .as_slice()
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

    set.evaluate(price(100), Nanoseconds::from_secs(4)).unwrap();
    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(5)),
        Err(Error::Tripped {
            breaker_ids: vec![id]
        })
    );

    assert!(matches!(
        set.breakers.get(&0).unwrap().status,
        CircuitBreakerStatus::Tripped {
            tripped_at_ns,
            price_update,
        } if tripped_at_ns == Nanoseconds::from_secs(5)
            && price_update.price == price(200)
            && price_update.observed_at_ns == Nanoseconds::from_secs(5)
    ));
}
