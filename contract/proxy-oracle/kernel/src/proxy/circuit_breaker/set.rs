use alloc::collections::BTreeMap;
use alloc::vec::Vec;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Nanoseconds;

use crate::Price;

use super::{CircuitBreaker, CircuitBreakerStatus, Error, Observation, RingBuffer};

serialize! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    /// Shared sampling configuration for a circuit breaker set.
    pub struct CircuitBreakerSetConfig {
        /// Minimum elapsed time between persisted observations.
        ///
        /// A value of zero persists every resolved proxy price. Rules still evaluate every
        /// observation against the proposed history regardless of whether the sample is persisted.
        pub sample_interval_ns: Nanoseconds,
        /// Maximum number of observations retained by the set.
        ///
        /// A value of zero is a coherent no-op history configuration: observations are not
        /// retained, so breakers that need prior samples cannot trip until history capacity is
        /// raised and enough observations have accumulated.
        pub history_len: u32,
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CircuitBreakerSet {
        pub sample_interval_ns: Nanoseconds,
        pub history: RingBuffer<Observation>,
        pub next_id: u32,
        pub is_manually_tripped: bool,
        pub breakers: BTreeMap<u32, CircuitBreakerState>,
    }
}

impl CircuitBreakerSet {
    #[must_use]
    /// Returns an empty, no-op set with zero retained history.
    ///
    /// Breakers can still be added later, but history-dependent breakers cannot trip until the
    /// set is configured with enough history capacity and samples have accumulated.
    pub fn empty() -> Self {
        Self::new(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 0,
        })
    }

    #[must_use]
    pub fn new(config: CircuitBreakerSetConfig) -> Self {
        Self {
            sample_interval_ns: config.sample_interval_ns,
            history: RingBuffer::new(config.history_len),
            next_id: 0,
            is_manually_tripped: false,
            breakers: BTreeMap::new(),
        }
    }

    pub fn set_config(&mut self, config: CircuitBreakerSetConfig) {
        self.sample_interval_ns = config.sample_interval_ns;
        self.history.set_capacity(config.history_len);
    }

    pub fn set_manual_trip(&mut self, is_manually_tripped: bool) {
        self.is_manually_tripped = is_manually_tripped;
    }

    pub fn add(&mut self, breaker_id: u32, breaker: CircuitBreaker) -> Result<(), Error> {
        if breaker_id != self.next_id {
            return Err(Error::UnexpectedBreakerId {
                expected: self.next_id,
                actual: breaker_id,
            });
        }

        self.next_id = self.next_id.checked_add(1).ok_or(Error::TooManyBreakers)?;
        self.breakers
            .insert(breaker_id, CircuitBreakerState::new(breaker));
        Ok(())
    }

    pub fn remove(&mut self, breaker_id: u32) -> Result<(), Error> {
        self.breakers
            .remove(&breaker_id)
            .ok_or(Error::BreakerNotFound { breaker_id })?;
        Ok(())
    }

    pub fn get_mut(&mut self, breaker_id: u32) -> Result<&mut CircuitBreakerState, Error> {
        self.breakers
            .get_mut(&breaker_id)
            .ok_or(Error::BreakerNotFound { breaker_id })
    }

    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.is_manually_tripped || self.breakers.values().any(CircuitBreakerState::is_blocking)
    }

    pub fn evaluate(&mut self, price: Price, now: Nanoseconds) -> Result<(), Error> {
        let mut proposed_history = self.history.clone();
        let price_update = Observation {
            price,
            observed_at_ns: now,
        };
        proposed_history.push(price_update);

        if self.should_persist_sample(now) {
            self.history = proposed_history.clone();
        }

        if self.is_manually_tripped {
            return Err(Error::ManuallyTripped);
        }

        let mut breaker_ids = Vec::new();

        for (breaker_id, breaker) in &mut self.breakers {
            let can_trip = match breaker.status {
                CircuitBreakerStatus::Armed => true,
                CircuitBreakerStatus::Muted { until_ns } if now >= until_ns => {
                    breaker.status = CircuitBreakerStatus::Armed;
                    true
                }
                CircuitBreakerStatus::Muted { .. } | CircuitBreakerStatus::Tripped { .. } => false,
            };

            if can_trip && breaker.breaker.should_trip(&proposed_history) {
                breaker.status = CircuitBreakerStatus::Tripped {
                    tripped_at_ns: now,
                    price_update,
                };
            }

            if breaker.is_blocking() {
                breaker_ids.push(*breaker_id);
            }
        }

        if breaker_ids.is_empty() {
            Ok(())
        } else {
            Err(Error::Tripped { breaker_ids })
        }
    }

    fn should_persist_sample(&self, now: Nanoseconds) -> bool {
        self.history
            .last()
            .is_none_or(|last| now.saturating_sub(last.observed_at_ns) >= self.sample_interval_ns)
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CircuitBreakerState {
        pub breaker: CircuitBreaker,
        pub is_enforced: bool,
        pub status: CircuitBreakerStatus,
    }
}

impl CircuitBreakerState {
    #[must_use]
    pub fn new(breaker: CircuitBreaker) -> Self {
        Self {
            breaker,
            is_enforced: true,
            status: CircuitBreakerStatus::Armed,
        }
    }

    pub fn is_blocking(&self) -> bool {
        self.is_enforced && matches!(self.status, CircuitBreakerStatus::Tripped { .. })
    }
}
