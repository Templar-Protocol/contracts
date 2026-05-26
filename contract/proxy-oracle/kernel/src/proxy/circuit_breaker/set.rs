use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::format;
#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::string::ToString;
#[cfg(feature = "schemars")]
use alloc::{borrow::ToOwned, boxed::Box};
use templar_primitives::Nanoseconds;

use crate::{primitive::AccountId, Price};

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
        /// A value of zero persists every resolved proxy price. Rules evaluate sampled candidates
        /// against proposed accepted history unless the set is already manually or breaker-tripped.
        pub sample_interval_ns: Nanoseconds,
        /// Maximum number of observations retained by the set.
        ///
        /// A value of zero is a coherent no-op history configuration: observations are not
        /// retained, so breakers that need prior samples cannot trip until history capacity is
        /// raised and enough accepted observations have accumulated.
        pub history_len: u32,
    }
}

serialize! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AcceptedHistorySource {
        Empty,
        Observed,
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CircuitBreakerEvent {
        ManualTripSet {
            is_manually_tripped: bool,
            actor: AccountId,
            metadata: Option<Vec<u8>>,
        },
        ConfigSet {
            config: CircuitBreakerSetConfig,
        },
        Added {
            breaker_id: u32,
            breaker: CircuitBreaker,
        },
        Removed {
            breaker_id: u32,
        },
        EnforcementSet {
            breaker_id: u32,
            is_enforced: bool,
        },
        Rearmed {
            breaker_id: u32,
            armed_after_ns: Nanoseconds,
            accepted_history_source: AcceptedHistorySource,
        },
        Tripped {
            breaker_id: u32,
            tripped_at_ns: Nanoseconds,
            price_update: Observation,
            is_enforced: bool,
        },
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CircuitBreakerOutcome<T = ()> {
        pub value: T,
        pub events: Vec<CircuitBreakerEvent>,
    }
}

impl<T> CircuitBreakerOutcome<T> {
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            events: Vec::new(),
        }
    }

    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> CircuitBreakerOutcome<U> {
        CircuitBreakerOutcome {
            value: f(self.value),
            events: self.events,
        }
    }

    #[must_use]
    pub fn with_events(self, events: Vec<CircuitBreakerEvent>) -> Self {
        Self { events, ..self }
    }
}

impl CircuitBreakerOutcome<()> {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            value: (),
            events: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_value<T>(self, value: T) -> CircuitBreakerOutcome<T> {
        CircuitBreakerOutcome {
            value,
            events: self.events,
        }
    }
}

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PriceBlockedReason {
        ManuallyTripped,
        BreakerTripped { blocking_breaker_ids: Vec<u32> },
    }
}

pub type PriceAcceptance = Result<Price, PriceBlockedReason>;

serialize! {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UncheckedCircuitBreakerSet<R = CircuitBreaker> {
        pub sample_interval_ns: Nanoseconds,
        pub accepted_history: RingBuffer<Observation>,
        pub observed_history: RingBuffer<Observation>,
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
    HistoryCapacityMismatch,
}

impl core::fmt::Display for CircuitBreakerSetParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BreakerIdOutOfRange => write!(f, "circuit breaker ID is out of range"),
            Self::HistoryCapacityMismatch => {
                write!(f, "circuit breaker histories have mismatched capacities")
            }
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
        if value.accepted_history.capacity() != value.observed_history.capacity() {
            return Err(CircuitBreakerSetParseError::HistoryCapacityMismatch);
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
            accepted_history: RingBuffer::new(config.history_len),
            observed_history: RingBuffer::new(config.history_len),
            next_id: 0,
            is_manually_tripped: false,
            breakers: BTreeMap::new(),
        })
    }

    pub fn set_config(&mut self, config: CircuitBreakerSetConfig) -> CircuitBreakerOutcome {
        self.0.sample_interval_ns = config.sample_interval_ns;
        self.0.accepted_history.set_capacity(config.history_len);
        self.0.observed_history.set_capacity(config.history_len);
        CircuitBreakerOutcome::empty().with_events(vec![CircuitBreakerEvent::ConfigSet { config }])
    }

    pub fn set_manual_trip(
        &mut self,
        is_manually_tripped: bool,
        actor: AccountId,
        metadata: Option<Vec<u8>>,
    ) -> CircuitBreakerOutcome {
        if self.0.is_manually_tripped == is_manually_tripped {
            return CircuitBreakerOutcome::empty();
        }

        self.set_manual_trip_state(is_manually_tripped);
        CircuitBreakerOutcome::empty().with_events(vec![CircuitBreakerEvent::ManualTripSet {
            is_manually_tripped,
            actor,
            metadata,
        }])
    }

    fn set_manual_trip_state(&mut self, is_manually_tripped: bool) {
        self.0.is_manually_tripped = is_manually_tripped;
    }

    fn add_state(&mut self, breaker_id: u32, breaker: R) -> Result<(), CircuitBreakerError> {
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

    pub fn remove(
        &mut self,
        breaker_id: u32,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.remove_state(breaker_id)?;
        Ok(CircuitBreakerOutcome::empty()
            .with_events(vec![CircuitBreakerEvent::Removed { breaker_id }]))
    }

    fn remove_state(&mut self, breaker_id: u32) -> Result<(), CircuitBreakerError> {
        self.0
            .breakers
            .remove(&breaker_id)
            .ok_or(CircuitBreakerError::BreakerNotFound { breaker_id })?;
        Ok(())
    }

    fn get_mut(
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
    pub fn accepted_history(&self) -> &RingBuffer<Observation> {
        &self.0.accepted_history
    }

    #[must_use]
    pub fn observed_history(&self) -> &RingBuffer<Observation> {
        &self.0.observed_history
    }

    fn clear_accepted_history(&mut self) {
        self.0.accepted_history.clear();
    }

    fn seed_accepted_history_from_observed(&mut self) {
        self.0.accepted_history = self.0.observed_history.clone();
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
            .observed_history
            .last()
            .is_none_or(|last| now.saturating_sub(last.observed_at_ns) >= self.0.sample_interval_ns)
    }

    fn blocking_breaker_ids(&self) -> Vec<u32> {
        self.0
            .breakers
            .iter()
            .filter_map(|(id, breaker)| breaker.is_blocking().then_some(*id))
            .collect()
    }
}

impl CircuitBreakerSet<CircuitBreaker> {
    pub fn add(
        &mut self,
        breaker_id: u32,
        breaker: CircuitBreaker,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        self.add_state(breaker_id, breaker.clone())?;
        Ok(
            CircuitBreakerOutcome::empty().with_events(vec![CircuitBreakerEvent::Added {
                breaker_id,
                breaker,
            }]),
        )
    }

    pub fn set_enforced(
        &mut self,
        breaker_id: u32,
        is_enforced: bool,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        let breaker = self.get_mut(breaker_id)?;
        if breaker.is_enforced == is_enforced {
            return Ok(CircuitBreakerOutcome::empty());
        }
        breaker.is_enforced = is_enforced;
        Ok(
            CircuitBreakerOutcome::empty().with_events(vec![CircuitBreakerEvent::EnforcementSet {
                breaker_id,
                is_enforced,
            }]),
        )
    }

    pub fn rearm(
        &mut self,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    ) -> Result<CircuitBreakerOutcome, CircuitBreakerError> {
        let breaker = self.get_mut(breaker_id)?;
        breaker.status = CircuitBreakerStatus::ArmedAfter {
            timestamp_ns: armed_after_ns,
        };
        match accepted_history_source {
            AcceptedHistorySource::Empty => self.clear_accepted_history(),
            AcceptedHistorySource::Observed => self.seed_accepted_history_from_observed(),
        }
        Ok(
            CircuitBreakerOutcome::empty().with_events(vec![CircuitBreakerEvent::Rearmed {
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            }]),
        )
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
    pub fn try_accept_price(
        &mut self,
        price: Price,
        now: Nanoseconds,
    ) -> Result<CircuitBreakerOutcome<PriceAcceptance>, CircuitBreakerError> {
        if !price.has_strictly_positive_confidence_interval() {
            return Err(CircuitBreakerError::InvalidPrice);
        }

        let mut proposed_accepted_history = self.0.accepted_history.clone();
        let price_update = Observation {
            price,
            observed_at_ns: now,
        };
        proposed_accepted_history.push(price_update);

        let should_persist_sample = self.should_persist_sample(now);
        if should_persist_sample {
            self.0.observed_history.push(price_update);
        }

        if self.0.is_manually_tripped {
            return Ok(CircuitBreakerOutcome::new(Err(
                PriceBlockedReason::ManuallyTripped,
            )));
        }

        let blocking_breaker_ids = self.blocking_breaker_ids();

        // Short-circuit in the case of already-blocking breakers: do not update
        // accepted_history or test untripped breakers against a stale accepted_history.
        if !blocking_breaker_ids.is_empty() {
            return Ok(CircuitBreakerOutcome::new(Err(
                PriceBlockedReason::BreakerTripped {
                    blocking_breaker_ids,
                },
            )));
        }

        let acceptance =
            self.apply_armed_breaker_transitions(&proposed_accepted_history, price_update, now);

        if acceptance.value.is_ok() && should_persist_sample {
            self.0.accepted_history = proposed_accepted_history;
        }
        Ok(acceptance)
    }

    fn apply_armed_breaker_transitions(
        &mut self,
        proposed_accepted_history: &RingBuffer<Observation>,
        price_update: Observation,
        now: Nanoseconds,
    ) -> CircuitBreakerOutcome<PriceAcceptance> {
        let mut events = vec![];

        for (breaker_id, breaker) in &mut self.0.breakers {
            if breaker.is_armed_at(now) && breaker.breaker.should_trip(proposed_accepted_history) {
                events.push(breaker.trip(*breaker_id, price_update, now));
            }

            if breaker.is_blocking() {
                return CircuitBreakerOutcome::new(Err(PriceBlockedReason::BreakerTripped {
                    blocking_breaker_ids: vec![*breaker_id],
                }))
                .with_events(events);
            }
        }

        CircuitBreakerOutcome::new(Ok(price_update.price)).with_events(events)
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

impl<R: CircuitBreakerRule> CircuitBreakerState<R> {
    fn is_armed_at(&self, now: Nanoseconds) -> bool {
        matches!(
            self.status,
            CircuitBreakerStatus::ArmedAfter { timestamp_ns } if now >= timestamp_ns
        )
    }

    fn trip(
        &mut self,
        breaker_id: u32,
        price_update: Observation,
        now: Nanoseconds,
    ) -> CircuitBreakerEvent {
        self.status = CircuitBreakerStatus::Tripped {
            tripped_at_ns: now,
            price_update,
        };
        CircuitBreakerEvent::Tripped {
            breaker_id,
            tripped_at_ns: now,
            price_update,
            is_enforced: self.is_enforced,
        }
    }
}
