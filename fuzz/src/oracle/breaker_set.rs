//! Reference model of `CircuitBreakerSet`, expressed in the kernel's own data types so a fuzz
//! target can compare whole states and outcomes with `assert_eq!`.

use std::collections::{BTreeMap, VecDeque};

use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    primitive::AccountId,
    proxy::circuit_breaker::{
        CircuitBreaker, CircuitBreakerError, CircuitBreakerEvent, CircuitBreakerOutcome,
        CircuitBreakerSet, CircuitBreakerSetConfig, CircuitBreakerState, CircuitBreakerStatus,
        Observation, PriceAcceptance, PriceBlockedReason,
    },
    Price,
};

use super::{is_valid_for, is_valid_price, should_trip};

type Outcome<T = ()> = Result<CircuitBreakerOutcome<T>, CircuitBreakerError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSet {
    config: CircuitBreakerSetConfig,
    accepted_history: VecDeque<Observation>,
    observed_history: VecDeque<Observation>,
    next_id: u32,
    is_manually_tripped: bool,
    breakers: BTreeMap<u32, CircuitBreakerState>,
}

impl From<&CircuitBreakerSet> for ModelSet {
    fn from(set: &CircuitBreakerSet) -> Self {
        Self {
            config: CircuitBreakerSetConfig {
                sample_interval_ns: set.sample_interval_ns(),
                history_len: set.accepted_history().capacity(),
            },
            accepted_history: set.accepted_history().iter().copied().collect(),
            observed_history: set.observed_history().iter().copied().collect(),
            next_id: set.next_id(),
            is_manually_tripped: set.is_manually_tripped(),
            breakers: set.breakers().clone(),
        }
    }
}

fn outcome<T>(value: T, events: Vec<CircuitBreakerEvent>) -> CircuitBreakerOutcome<T> {
    CircuitBreakerOutcome { value, events }
}

fn push_bounded(history: &mut VecDeque<Observation>, observation: Observation, capacity: u32) {
    history.push_back(observation);
    truncate_oldest(history, capacity);
}

fn truncate_oldest(history: &mut VecDeque<Observation>, capacity: u32) {
    let excess = history
        .len()
        .saturating_sub(usize::try_from(capacity).unwrap_or(usize::MAX));
    history.drain(..excess);
}

fn advances(history: &VecDeque<Observation>, price: &Price) -> bool {
    history
        .back()
        .is_none_or(|last| price.publish_time_ns > last.price.publish_time_ns)
}

fn is_armed_at(state: &CircuitBreakerState, now: Nanoseconds) -> bool {
    matches!(state.status, CircuitBreakerStatus::ArmedAfter { timestamp_ns } if now >= timestamp_ns)
}

impl ModelSet {
    pub fn new(config: CircuitBreakerSetConfig) -> Self {
        Self {
            config,
            accepted_history: VecDeque::new(),
            observed_history: VecDeque::new(),
            next_id: 0,
            is_manually_tripped: false,
            breakers: BTreeMap::new(),
        }
    }

    pub fn set_manual_trip(
        &mut self,
        is_manually_tripped: bool,
        actor: AccountId,
        metadata: Option<Vec<u8>>,
    ) -> CircuitBreakerOutcome {
        if self.is_manually_tripped == is_manually_tripped {
            return outcome((), vec![]);
        }
        self.is_manually_tripped = is_manually_tripped;
        outcome(
            (),
            vec![CircuitBreakerEvent::ManualTripSet {
                is_manually_tripped,
                actor,
                metadata,
            }],
        )
    }

    pub fn add(&mut self, breaker_id: u32, breaker: CircuitBreaker) -> Outcome {
        if !is_valid_for(
            &breaker,
            self.config.sample_interval_ns,
            self.config.history_len,
        ) {
            return Err(CircuitBreakerError::InvalidConfiguration);
        }
        if breaker_id != self.next_id {
            return Err(CircuitBreakerError::UnexpectedBreakerId {
                expected: self.next_id,
                actual: breaker_id,
            });
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(CircuitBreakerError::TooManyBreakers)?;
        self.breakers.insert(
            breaker_id,
            CircuitBreakerState {
                breaker: breaker.clone(),
                is_enforced: true,
                status: CircuitBreakerStatus::ArmedAfter {
                    timestamp_ns: Nanoseconds::zero(),
                },
            },
        );
        Ok(outcome(
            (),
            vec![CircuitBreakerEvent::Added {
                breaker_id,
                breaker,
            }],
        ))
    }

    pub fn remove(&mut self, breaker_id: u32) -> Outcome {
        self.breakers
            .remove(&breaker_id)
            .ok_or(CircuitBreakerError::BreakerNotFound { breaker_id })?;
        Ok(outcome(
            (),
            vec![CircuitBreakerEvent::Removed { breaker_id }],
        ))
    }

    pub fn set_enforced(&mut self, breaker_id: u32, is_enforced: bool) -> Outcome {
        let state = self.state_mut(breaker_id)?;
        if state.is_enforced == is_enforced {
            return Ok(outcome((), vec![]));
        }
        state.is_enforced = is_enforced;
        Ok(outcome(
            (),
            vec![CircuitBreakerEvent::EnforcementSet {
                breaker_id,
                is_enforced,
            }],
        ))
    }

    pub fn rearm(&mut self, breaker_id: u32, armed_after_ns: Nanoseconds) -> Outcome {
        let status = CircuitBreakerStatus::ArmedAfter {
            timestamp_ns: armed_after_ns,
        };
        let state = self.state_mut(breaker_id)?;
        if state.status == status {
            return Ok(outcome((), vec![]));
        }
        state.status = status;
        Ok(outcome(
            (),
            vec![CircuitBreakerEvent::Rearmed {
                breaker_id,
                armed_after_ns,
            }],
        ))
    }

    pub fn set_config(&mut self, config: CircuitBreakerSetConfig) -> Outcome {
        if self.breakers.values().any(|state| {
            !is_valid_for(
                &state.breaker,
                config.sample_interval_ns,
                config.history_len,
            )
        }) {
            return Err(CircuitBreakerError::InvalidConfiguration);
        }
        if config == self.config {
            return Ok(outcome((), vec![]));
        }
        self.config = config;
        truncate_oldest(&mut self.accepted_history, config.history_len);
        truncate_oldest(&mut self.observed_history, config.history_len);
        Ok(outcome((), vec![CircuitBreakerEvent::ConfigSet { config }]))
    }

    /// Records `price` as observed when sampled, evaluates every armed breaker against accepted
    /// history extended by `price`, and accepts it only if no enforced breaker ends up tripped.
    pub fn try_accept_price(&mut self, price: Price, now: Nanoseconds) -> Outcome<PriceAcceptance> {
        if !is_valid_price(&price) {
            return Err(CircuitBreakerError::InvalidPrice);
        }
        let observation = Observation {
            price,
            observed_at_ns: now,
        };
        let capacity = self.config.history_len;
        let is_sampled = self.observed_history.back().is_none_or(|last| {
            now.saturating_sub(last.observed_at_ns) >= self.config.sample_interval_ns
        });
        if is_sampled && advances(&self.observed_history, &price) {
            push_bounded(&mut self.observed_history, observation, capacity);
        }

        if self.is_manually_tripped {
            return Ok(outcome(Err(PriceBlockedReason::ManuallyTripped), vec![]));
        }
        if let Some(reason) = self.breaker_block() {
            return Ok(outcome(Err(reason), vec![]));
        }

        let mut proposed_history = self.accepted_history.clone();
        push_bounded(&mut proposed_history, observation, capacity);
        let proposed_prices = proposed_history
            .iter()
            .map(|observation| observation.price)
            .collect::<Vec<_>>();
        let mut events = vec![];
        for (breaker_id, state) in &mut self.breakers {
            if is_armed_at(state, now) && should_trip(&state.breaker, &proposed_prices) {
                state.status = CircuitBreakerStatus::Tripped {
                    tripped_at_ns: now,
                    price_update: observation,
                };
                events.push(CircuitBreakerEvent::Tripped {
                    breaker_id: *breaker_id,
                    tripped_at_ns: now,
                    price_update: observation,
                    is_enforced: state.is_enforced,
                });
            }
        }

        let value = self.breaker_block().map_or(Ok(price), Err);
        if value.is_ok() && is_sampled && advances(&self.accepted_history, &price) {
            self.accepted_history = proposed_history;
        }
        Ok(outcome(value, events))
    }

    fn breaker_block(&self) -> Option<PriceBlockedReason> {
        let blocking_breaker_ids = self
            .breakers
            .iter()
            .filter(|(_, state)| {
                state.is_enforced && matches!(state.status, CircuitBreakerStatus::Tripped { .. })
            })
            .map(|(breaker_id, _)| *breaker_id)
            .collect::<Vec<_>>();
        (!blocking_breaker_ids.is_empty()).then_some(PriceBlockedReason::BreakerTripped {
            blocking_breaker_ids,
        })
    }

    fn state_mut(
        &mut self,
        breaker_id: u32,
    ) -> Result<&mut CircuitBreakerState, CircuitBreakerError> {
        self.breakers
            .get_mut(&breaker_id)
            .ok_or(CircuitBreakerError::BreakerNotFound { breaker_id })
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use templar_primitives::Decimal;
    use templar_proxy_oracle_kernel::proxy::circuit_breaker::{MonotonicRun, StepwiseChange};

    use super::*;

    const ACTOR: AccountId = AccountId::from_bytes([0; 64]);

    fn price(mantissa: i64, publish_time_s: u64) -> Price {
        Price {
            price: mantissa,
            conf: 0,
            expo: -2,
            publish_time_ns: Nanoseconds::from_secs(publish_time_s),
        }
    }

    fn stepwise(threshold: &str) -> CircuitBreaker {
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: Decimal::from_str(threshold)
                .unwrap_or_else(|_| panic!("invalid decimal {threshold}")),
        })
    }

    /// Applies one operation to both the kernel and the model, requiring identical results.
    struct Pair {
        kernel: CircuitBreakerSet,
        model: ModelSet,
    }

    impl Pair {
        fn new(sample_interval_s: u64, history_len: u32) -> Self {
            let config = CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::from_secs(sample_interval_s),
                history_len,
            };
            Self {
                kernel: CircuitBreakerSet::new(config),
                model: ModelSet::new(config),
            }
        }

        fn accept(&mut self, price: Price, now_s: u64) -> Outcome<PriceAcceptance> {
            let now = Nanoseconds::from_secs(now_s);
            let expected = self.model.try_accept_price(price, now);
            assert_eq!(self.kernel.try_accept_price(price, now), expected);
            assert_eq!(ModelSet::from(&self.kernel), self.model);
            expected
        }

        fn apply(
            &mut self,
            kernel: impl FnOnce(&mut CircuitBreakerSet) -> Outcome,
            model: impl FnOnce(&mut ModelSet) -> Outcome,
        ) -> Outcome {
            let expected = model(&mut self.model);
            assert_eq!(kernel(&mut self.kernel), expected);
            assert_eq!(ModelSet::from(&self.kernel), self.model);
            expected
        }
    }

    #[test]
    fn tripped_breaker_blocks_while_observed_history_advances() {
        let mut pair = Pair::new(0, 4);
        pair.apply(
            |set| set.add(0, stepwise("0.1")),
            |set| set.add(0, stepwise("0.1")),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            pair.accept(price(100, 1), 1).map(|outcome| outcome.value),
            Ok(Ok(price(100, 1)))
        );
        let tripped = pair.accept(price(120, 2), 2);
        assert!(matches!(
            tripped,
            Ok(CircuitBreakerOutcome { value: Err(PriceBlockedReason::BreakerTripped { .. }), ref events })
                if events.len() == 1
        ));
        let blocked = pair.accept(price(101, 3), 3);
        assert!(
            matches!(blocked, Ok(CircuitBreakerOutcome { ref events, .. }) if events.is_empty())
        );
        assert_eq!(pair.model.accepted_history.len(), 1);
        assert_eq!(pair.model.observed_history.len(), 3);

        pair.apply(
            |set| set.rearm(0, Nanoseconds::zero()),
            |set| set.rearm(0, Nanoseconds::zero()),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        pair.apply(
            |set| Ok(set.set_manual_trip(true, ACTOR, None)),
            |set| Ok(set.set_manual_trip(true, ACTOR, None)),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            pair.accept(price(102, 4), 4).map(|outcome| outcome.value),
            Ok(Err(PriceBlockedReason::ManuallyTripped))
        );
    }

    #[test]
    fn sampling_and_stale_publish_times_hold_history_back() {
        let mut pair = Pair::new(10, 3);
        pair.accept(price(100, 5), 100)
            .unwrap_or_else(|error| panic!("{error}"));
        pair.accept(price(101, 6), 105)
            .unwrap_or_else(|error| panic!("{error}"));
        pair.accept(price(102, 6), 120)
            .unwrap_or_else(|error| panic!("{error}"));
        pair.accept(price(103, 6), 130)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(pair.model.accepted_history.len(), 2);
    }

    #[test]
    fn reconfiguration_rejects_rules_it_would_invalidate_and_truncates_history() {
        let mut pair = Pair::new(0, 4);
        let monotonic = CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 3,
            min_relative_step_change: Decimal::ONE,
        });
        pair.apply(
            |set| set.add(0, monotonic.clone()),
            |set| set.add(0, monotonic.clone()),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for second in 1..=4 {
            pair.accept(price(100, second), second)
                .unwrap_or_else(|error| panic!("{error}"));
        }
        let sampled = CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::from_secs(1),
            history_len: 4,
        };
        assert_eq!(
            pair.apply(|set| set.set_config(sampled), |set| set.set_config(sampled)),
            Err(CircuitBreakerError::InvalidConfiguration)
        );
        pair.apply(|set| set.remove(0), |set| set.remove(0))
            .unwrap_or_else(|error| panic!("{error}"));
        pair.apply(|set| set.set_config(sampled), |set| set.set_config(sampled))
            .unwrap_or_else(|error| panic!("{error}"));
        let shrunk = CircuitBreakerSetConfig {
            history_len: 2,
            ..sampled
        };
        pair.apply(|set| set.set_config(shrunk), |set| set.set_config(shrunk))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(pair.model.accepted_history.len(), 2);
    }
}
