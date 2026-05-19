use near_sdk::{json_types::Base64VecU8, near, AccountId};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::Observation;

use crate::role::Role;

pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;

#[near(event_json(standard = "templar-proxy-oracle"))]
pub enum Event {
    #[event_version("1.0.0")]
    CircuitBreakerRoleSet {
        account_id: AccountId,
        role: Role,
        is_granted: bool,
    },
    #[event_version("1.0.0")]
    CircuitBreakerManualTripSet {
        price_id: PriceIdentifier,
        is_manually_tripped: bool,
        actor: AccountId,
        metadata: Option<Base64VecU8>,
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
