use near_sdk::{env, json_types::Base64VecU8, near, AccountId};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    AcceptedHistorySource, CircuitBreaker, CircuitBreakerEvent as KernelEvent,
    CircuitBreakerSetConfig, Observation,
};

use crate::convert::account_id_try_from_kernel;

pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;

#[near(event_json(standard = "templar-proxy-oracle"))]
pub enum Event {
    #[event_version("1.0.0")]
    CircuitBreakerManualTripSet {
        price_id: PriceIdentifier,
        is_manually_tripped: bool,
        actor: AccountId,
        metadata: Option<Base64VecU8>,
    },
    #[event_version("1.0.0")]
    CircuitBreakerConfigSet {
        price_id: PriceIdentifier,
        config: CircuitBreakerSetConfig,
    },
    #[event_version("1.0.0")]
    CircuitBreakerAdded {
        price_id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    },
    #[event_version("1.0.0")]
    CircuitBreakerRemoved {
        price_id: PriceIdentifier,
        breaker_id: u32,
    },
    #[event_version("1.0.0")]
    CircuitBreakerEnforcementSet {
        price_id: PriceIdentifier,
        breaker_id: u32,
        is_enforced: bool,
    },
    #[event_version("1.0.0")]
    CircuitBreakerRearmed {
        price_id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
        accepted_history_source: AcceptedHistorySource,
    },
    #[event_version("1.0.0")]
    CircuitBreakerTripped {
        price_id: PriceIdentifier,
        breaker_id: u32,
        tripped_at_ns: Nanoseconds,
        price_update: Observation,
        is_enforced: bool,
    },
}

impl Event {
    // Return type is Self; Option cannot be propagated. The kernel only stores
    // valid NEAR account IDs, so None here is an invariant violation.
    #[allow(clippy::expect_used)]
    pub fn from_kernel(price_id: PriceIdentifier, event: KernelEvent) -> Self {
        match event {
            KernelEvent::ManualTripSet {
                is_manually_tripped,
                actor,
                metadata,
            } => Self::CircuitBreakerManualTripSet {
                price_id,
                is_manually_tripped,
                actor: account_id_try_from_kernel(actor).unwrap_or_else(|| {
                    env::panic_str("kernel account ID must contain a valid NEAR account ID")
                }),
                metadata: metadata.map(Base64VecU8),
            },
            KernelEvent::ConfigSet { config } => Self::CircuitBreakerConfigSet { price_id, config },
            KernelEvent::Added {
                breaker_id,
                breaker,
            } => Self::CircuitBreakerAdded {
                price_id,
                breaker_id,
                breaker,
            },
            KernelEvent::Removed { breaker_id } => Self::CircuitBreakerRemoved {
                price_id,
                breaker_id,
            },
            KernelEvent::EnforcementSet {
                breaker_id,
                is_enforced,
            } => Self::CircuitBreakerEnforcementSet {
                price_id,
                breaker_id,
                is_enforced,
            },
            KernelEvent::Rearmed {
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            } => Self::CircuitBreakerRearmed {
                price_id,
                breaker_id,
                armed_after_ns,
                accepted_history_source,
            },
            KernelEvent::Tripped {
                breaker_id,
                tripped_at_ns,
                price_update,
                is_enforced,
            } => Self::CircuitBreakerTripped {
                price_id,
                breaker_id,
                tripped_at_ns,
                price_update,
                is_enforced,
            },
        }
    }
}
