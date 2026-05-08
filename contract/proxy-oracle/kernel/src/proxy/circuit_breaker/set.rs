use alloc::collections::BTreeMap;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Nanoseconds;

use crate::Price;

use super::{
    CircuitBreaker, CircuitBreakerStatus, CircuitBreakerStatusUpdate, Error, Observation,
    RingBuffer,
};

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CircuitBreakerSetConfig {
        pub sample_interval_ns: Nanoseconds,
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

    pub fn add(&mut self, order: u32, breaker: CircuitBreaker) -> Result<u32, Error> {
        if self.breakers.contains_key(&order) {
            return Err(Error::OrderOccupied { order });
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.breakers
            .insert(order, CircuitBreakerState::new(id, breaker));
        Ok(id)
    }

    pub fn remove(&mut self, breaker_id: u32) -> Result<(), Error> {
        let order = self
            .breakers
            .iter()
            .find_map(|(order, breaker)| (breaker.id == breaker_id).then_some(*order))
            .ok_or(Error::BreakerNotFound { breaker_id })?;
        self.breakers.remove(&order);
        Ok(())
    }

    pub fn set_status(
        &mut self,
        breaker_id: u32,
        status: CircuitBreakerStatusUpdate,
    ) -> Result<(), Error> {
        let breaker = self
            .breakers
            .values_mut()
            .find(|breaker| breaker.id == breaker_id)
            .ok_or(Error::BreakerNotFound { breaker_id })?;

        match status {
            CircuitBreakerStatusUpdate::Enable => breaker.is_enabled = true,
            CircuitBreakerStatusUpdate::Disable => breaker.is_enabled = false,
            CircuitBreakerStatusUpdate::Arm => breaker.status = CircuitBreakerStatus::Armed,
            CircuitBreakerStatusUpdate::Mute { until_ns } => {
                breaker.status = CircuitBreakerStatus::Muted { until_ns };
            }
        }

        Ok(())
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

        let mut tripped_by = None;

        for breaker in self.breakers.values_mut() {
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
                tripped_by.get_or_insert(breaker.id);
            }
        }

        tripped_by.map_or(Ok(()), |breaker_id| Err(Error::Tripped { breaker_id }))
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
        pub id: u32,
        pub breaker: CircuitBreaker,
        pub is_enabled: bool,
        pub status: CircuitBreakerStatus,
    }
}

impl CircuitBreakerState {
    #[must_use]
    pub fn new(id: u32, breaker: CircuitBreaker) -> Self {
        Self {
            id,
            breaker,
            is_enabled: true,
            status: CircuitBreakerStatus::Armed,
        }
    }

    pub fn is_blocking(&self) -> bool {
        self.is_enabled && matches!(self.status, CircuitBreakerStatus::Tripped { .. })
    }
}
