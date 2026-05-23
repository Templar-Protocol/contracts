#![no_std]

use soroban_sdk::{contractevent, Address, Bytes};
use templar_proxy_oracle_soroban_common::{Asset, Role};

#[contractevent]
#[derive(Clone)]
pub struct RefreshSuccess {
    #[topic]
    pub asset: Asset,
    pub price: i128,
    pub timestamp: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct RefreshFailure {
    #[topic]
    pub asset: Asset,
    pub code: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CacheBlocked {
    #[topic]
    pub asset: Asset,
    pub reason_code: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerConfigSet {
    #[topic]
    pub asset: Asset,
    pub sample_interval_secs: u64,
    pub history_len: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerAdded {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub breaker_id: u32,
    pub breaker_kind: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerRemoved {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub breaker_id: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerEnforcementSet {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub breaker_id: u32,
    pub is_enforced: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerRearmed {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub breaker_id: u32,
    pub armed_after_secs: u64,
    pub accepted_history_source_code: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerTripped {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub breaker_id: u32,
    pub tripped_at_secs: u64,
    pub price: i128,
    pub timestamp: u64,
    pub is_enforced: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct ManualTripSet {
    #[topic]
    pub asset: Asset,
    #[topic]
    pub actor: Address,
    pub is_manually_tripped: bool,
    pub metadata: Option<Bytes>,
}

#[contractevent]
#[derive(Clone)]
pub struct CircuitBreakerRoleSet {
    #[topic]
    pub account: Address,
    pub role: Role,
    pub is_granted: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct ProxySet {
    #[topic]
    pub asset: Asset,
    pub source_count: u32,
    pub min_sources: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct ProxyRemoved {
    #[topic]
    pub asset: Asset,
}

#[contractevent]
#[derive(Clone)]
pub struct GovernanceHandoff {
    #[topic]
    pub old_governance: Address,
    #[topic]
    pub new_governance: Address,
}

#[contractevent]
#[derive(Clone)]
pub struct TtlExtended {
    pub asset_count: u32,
}
