use near_sdk::{json_types::Base64VecU8, near, AccountId};
use templar_common::oracle::pyth::PriceIdentifier;

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
}
