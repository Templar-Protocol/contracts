use alloc::collections::BTreeMap;
use alloc::vec::Vec;

#[cfg(feature = "schemars")]
use alloc::borrow::ToOwned;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::format;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
use templar_primitives::Nanoseconds;

use crate::Price;

use super::{
    CircuitBreaker, CircuitBreakerError, CircuitBreakerRule, CircuitBreakerStatus, Observation,
    RingBuffer,
};

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
    pub struct UncheckedCircuitBreakerSet<R = CircuitBreaker> {
        pub sample_interval_ns: Nanoseconds,
        pub history: RingBuffer<Observation>,
        pub next_id: u32,
        pub is_manually_tripped: bool,
        pub breakers: BTreeMap<u32, CircuitBreakerState<R>>,
    }
}

#[cfg_attr(
    feature = "serde",
    derive(::serde::Deserialize, ::serde::Serialize),
    serde(
        try_from = "UncheckedCircuitBreakerSet<R>",
        into = "UncheckedCircuitBreakerSet<R>",
        bound(
            serialize = "R: Clone + ::serde::Serialize",
            deserialize = "R: ::serde::Deserialize<'de>"
        )
    )
)]
#[cfg_attr(
    feature = "schemars",
    derive(::schemars::JsonSchema),
    schemars(transparent)
)]
#[cfg_attr(
    feature = "borsh",
    derive(::borsh::BorshSerialize, ::borsh::BorshSchema)
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitBreakerSet<R = CircuitBreaker>(UncheckedCircuitBreakerSet<R>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerSetParseError {
    BreakerIdOutOfRange,
}

impl core::fmt::Display for CircuitBreakerSetParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BreakerIdOutOfRange => write!(f, "circuit breaker ID is out of range"),
        }
    }
}

impl<R> TryFrom<UncheckedCircuitBreakerSet<R>> for CircuitBreakerSet<R> {
    type Error = CircuitBreakerSetParseError;

    fn try_from(value: UncheckedCircuitBreakerSet<R>) -> Result<Self, Self::Error> {
        if value
            .breakers
            .keys()
            .next_back()
            .is_some_and(|breaker_id| *breaker_id >= value.next_id)
        {
            return Err(CircuitBreakerSetParseError::BreakerIdOutOfRange);
        }
        Ok(Self(value))
    }
}

impl<R> From<CircuitBreakerSet<R>> for UncheckedCircuitBreakerSet<R> {
    fn from(value: CircuitBreakerSet<R>) -> Self {
        value.0
    }
}

impl<R> CircuitBreakerSet<R> {
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
        Self(UncheckedCircuitBreakerSet {
            sample_interval_ns: config.sample_interval_ns,
            history: RingBuffer::new(config.history_len),
            next_id: 0,
            is_manually_tripped: false,
            breakers: BTreeMap::new(),
        })
    }

    pub fn set_config(&mut self, config: CircuitBreakerSetConfig) {
        self.0.sample_interval_ns = config.sample_interval_ns;
        self.0.history.set_capacity(config.history_len);
    }

    pub fn set_manual_trip(&mut self, is_manually_tripped: bool) {
        self.0.is_manually_tripped = is_manually_tripped;
    }

    pub fn add(&mut self, breaker_id: u32, breaker: R) -> Result<(), CircuitBreakerError> {
        if breaker_id != self.0.next_id {
            return Err(CircuitBreakerError::UnexpectedBreakerId {
                expected: self.0.next_id,
                actual: breaker_id,
            });
        }

        self.0.next_id = self
            .0
            .next_id
            .checked_add(1)
            .ok_or(CircuitBreakerError::TooManyBreakers)?;
        self.0
            .breakers
            .insert(breaker_id, CircuitBreakerState::new(breaker));
        Ok(())
    }

    pub fn remove(&mut self, breaker_id: u32) -> Result<(), CircuitBreakerError> {
        self.0
            .breakers
            .remove(&breaker_id)
            .ok_or(CircuitBreakerError::BreakerNotFound { breaker_id })?;
        Ok(())
    }

    pub fn get_mut(
        &mut self,
        breaker_id: u32,
    ) -> Result<&mut CircuitBreakerState<R>, CircuitBreakerError> {
        self.0
            .breakers
            .get_mut(&breaker_id)
            .ok_or(CircuitBreakerError::BreakerNotFound { breaker_id })
    }

    #[must_use]
    pub fn sample_interval_ns(&self) -> Nanoseconds {
        self.0.sample_interval_ns
    }

    #[must_use]
    pub fn history(&self) -> &RingBuffer<Observation> {
        &self.0.history
    }

    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.0.next_id
    }

    #[must_use]
    pub fn is_manually_tripped(&self) -> bool {
        self.0.is_manually_tripped
    }

    #[must_use]
    pub fn breakers(&self) -> &BTreeMap<u32, CircuitBreakerState<R>> {
        &self.0.breakers
    }

    #[must_use]
    pub fn breaker_count(&self) -> usize {
        self.0.breakers.len()
    }

    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.0.is_manually_tripped
            || self
                .0
                .breakers
                .values()
                .any(CircuitBreakerState::is_blocking)
    }

    fn should_persist_sample(&self, now: Nanoseconds) -> bool {
        self.0
            .history
            .last()
            .is_none_or(|last| now.saturating_sub(last.observed_at_ns) >= self.0.sample_interval_ns)
    }
}

#[cfg(feature = "borsh")]
impl<R: ::borsh::BorshDeserialize> ::borsh::BorshDeserialize for CircuitBreakerSet<R> {
    fn deserialize_reader<Reader: ::borsh::io::Read>(
        reader: &mut Reader,
    ) -> ::borsh::io::Result<Self> {
        let unchecked =
            <UncheckedCircuitBreakerSet<R> as ::borsh::BorshDeserialize>::deserialize_reader(
                reader,
            )?;
        unchecked.try_into().map_err(|_| {
            ::borsh::io::Error::new(
                ::borsh::io::ErrorKind::InvalidData,
                "could not parse circuit breaker set",
            )
        })
    }
}

impl<R: CircuitBreakerRule> CircuitBreakerSet<R> {
    pub fn evaluate(&mut self, price: Price, now: Nanoseconds) -> Result<(), CircuitBreakerError> {
        let mut proposed_history = self.0.history.clone();
        let price_update = Observation {
            price,
            observed_at_ns: now,
        };
        proposed_history.push(price_update);

        if self.should_persist_sample(now) {
            self.0.history = proposed_history.clone();
        }

        if self.0.is_manually_tripped {
            return Err(CircuitBreakerError::ManuallyTripped);
        }

        let mut breaker_ids = Vec::new();

        for (breaker_id, breaker) in &mut self.0.breakers {
            let can_trip = match breaker.status {
                CircuitBreakerStatus::ArmedAfter { timestamp_ns } => now >= timestamp_ns,
                CircuitBreakerStatus::Tripped { .. } => false,
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
            Err(CircuitBreakerError::Tripped { breaker_ids })
        }
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CircuitBreakerState<R = CircuitBreaker> {
        pub breaker: R,
        pub is_enforced: bool,
        pub status: CircuitBreakerStatus,
    }
}

impl<R> CircuitBreakerState<R> {
    #[must_use]
    pub fn new(breaker: R) -> Self {
        Self {
            breaker,
            is_enforced: true,
            status: CircuitBreakerStatus::ArmedAfter {
                timestamp_ns: Nanoseconds::zero(),
            },
        }
    }

    pub fn is_blocking(&self) -> bool {
        self.is_enforced && matches!(self.status, CircuitBreakerStatus::Tripped { .. })
    }
}
