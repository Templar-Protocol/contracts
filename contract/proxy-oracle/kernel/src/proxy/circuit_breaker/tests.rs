use core::str::FromStr;

use alloc::{vec, vec::Vec};
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

    let id = set.add(0, breaker).unwrap();
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
fn set_rejects_occupied_order() {
    let mut set = CircuitBreakerSet::empty();
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: dec("0.10"),
    });
    set.add(0, breaker.clone()).unwrap();

    assert_eq!(set.add(0, breaker), Err(Error::OrderOccupied { order: 0 }));
}

#[test]
fn muted_breaker_records_history_without_tripping() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.01"),
            }),
        )
        .unwrap();
    set.set_status(
        id,
        CircuitBreakerStatusUpdate::Mute {
            until_ns: Nanoseconds::from_secs(10),
        },
    )
    .unwrap();

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
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.10"),
            }),
        )
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(111), Nanoseconds::from_secs(2)),
        Err(Error::Tripped { breaker_id: id })
    );
    assert_eq!(
        set.evaluate(price(111), Nanoseconds::from_secs(3)),
        Err(Error::Tripped { breaker_id: id })
    );
}

#[test]
fn too_soon_sample_can_trip_without_being_persisted() {
    let mut set = breaker_set(Nanoseconds::from_secs(10), 2);
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.10"),
            }),
        )
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(2)),
        Err(Error::Tripped { breaker_id: id })
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
    let disabled_id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.01"),
            }),
        )
        .unwrap();
    let tripped_id = set
        .add(
            1,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.01"),
            }),
        )
        .unwrap();

    set.set_status(disabled_id, CircuitBreakerStatusUpdate::Disable)
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();

    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(2)),
        Err(Error::Tripped {
            breaker_id: tripped_id,
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
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.10"),
            }),
        )
        .unwrap();

    set.set_status(id, CircuitBreakerStatusUpdate::Disable)
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    set.evaluate(price(120), Nanoseconds::from_secs(2)).unwrap();

    let breaker = set.breakers.get(&0).unwrap();
    assert!(!breaker.is_enabled);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::Tripped { .. }
    ));
    assert!(!set.is_blocking());

    set.set_status(id, CircuitBreakerStatusUpdate::Enable)
        .unwrap();

    assert!(set.is_blocking());
    assert_eq!(
        set.evaluate(price(130), Nanoseconds::from_secs(3)),
        Err(Error::Tripped { breaker_id: id })
    );
}

#[test]
fn arm_clears_tripped_status_without_enabling_breaker() {
    let mut set = breaker_set(Nanoseconds::zero(), 2);
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.10"),
            }),
        )
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(1)).unwrap();
    assert_eq!(
        set.evaluate(price(120), Nanoseconds::from_secs(2)),
        Err(Error::Tripped { breaker_id: id })
    );
    set.set_status(id, CircuitBreakerStatusUpdate::Disable)
        .unwrap();
    set.set_status(id, CircuitBreakerStatusUpdate::Arm).unwrap();

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
    let id = set
        .add(
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: dec("0.01"),
            }),
        )
        .unwrap();

    set.evaluate(price(100), Nanoseconds::from_secs(4)).unwrap();
    assert_eq!(
        set.evaluate(price(200), Nanoseconds::from_secs(5)),
        Err(Error::Tripped { breaker_id: id })
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
