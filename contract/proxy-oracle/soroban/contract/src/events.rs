//! Soroban event types and the helpers that translate kernel-emitted breaker
//! events / refresh outcomes into the publishable form.

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{contractevent, Address, Bytes, BytesN, Env};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreakerEvent as KernelCircuitBreakerEvent, Observation,
};
use templar_proxy_oracle_soroban_common::Asset;

use crate::{
    codes::{accepted_history_source_code, breaker_kind_code},
    RefreshStatus, SOURCE_UNAVAILABLE_CODE, UNKNOWN_ASSET_CODE,
};

#[contractevent]
#[derive(Clone)]
pub struct RefreshSuccess {
    #[topic]
    pub asset: Asset,
    pub mantissa: i64,
    pub expo: i32,
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
pub struct ContractUpgraded {
    pub new_wasm_hash: BytesN<32>,
}

#[contractevent]
#[derive(Clone)]
pub struct TtlExtended {
    pub asset_count: u32,
}

pub fn publish_refresh_event(env: &Env, asset: &Asset, status: &RefreshStatus) {
    match status {
        RefreshStatus::Accepted(price) => RefreshSuccess {
            asset: asset.clone(),
            mantissa: price.mantissa,
            expo: price.expo,
            timestamp: price.timestamp,
        }
        .publish(env),
        RefreshStatus::Blocked(reason_code) => CacheBlocked {
            asset: asset.clone(),
            reason_code: *reason_code,
        }
        .publish(env),
        RefreshStatus::ResolveFailed(code) => RefreshFailure {
            asset: asset.clone(),
            code: *code,
        }
        .publish(env),
        RefreshStatus::SourceUnavailable => RefreshFailure {
            asset: asset.clone(),
            code: SOURCE_UNAVAILABLE_CODE,
        }
        .publish(env),
        RefreshStatus::UnknownAsset => RefreshFailure {
            asset: asset.clone(),
            code: UNKNOWN_ASSET_CODE,
        }
        .publish(env),
    }
}

pub fn publish_breaker_events(
    env: &Env,
    asset: &Asset,
    events: AllocVec<KernelCircuitBreakerEvent>,
) {
    for event in events {
        publish_breaker_event(env, asset, event);
    }
}

fn publish_breaker_event(env: &Env, asset: &Asset, event: KernelCircuitBreakerEvent) {
    match event {
        // `ManualTripSet` is published from the runtime layer with the actor
        // address, not from the kernel event.
        KernelCircuitBreakerEvent::ManualTripSet { .. } => {}
        KernelCircuitBreakerEvent::ConfigSet { config } => CircuitBreakerConfigSet {
            asset: asset.clone(),
            sample_interval_secs: config.sample_interval_ns.as_secs(),
            history_len: config.history_len,
        }
        .publish(env),
        KernelCircuitBreakerEvent::Added {
            breaker_id,
            breaker,
        } => CircuitBreakerAdded {
            asset: asset.clone(),
            breaker_id,
            breaker_kind: breaker_kind_code(&breaker),
        }
        .publish(env),
        KernelCircuitBreakerEvent::Removed { breaker_id } => CircuitBreakerRemoved {
            asset: asset.clone(),
            breaker_id,
        }
        .publish(env),
        KernelCircuitBreakerEvent::EnforcementSet {
            breaker_id,
            is_enforced,
        } => CircuitBreakerEnforcementSet {
            asset: asset.clone(),
            breaker_id,
            is_enforced,
        }
        .publish(env),
        KernelCircuitBreakerEvent::Rearmed {
            breaker_id,
            armed_after_ns,
            accepted_history_source,
        } => CircuitBreakerRearmed {
            asset: asset.clone(),
            breaker_id,
            armed_after_secs: armed_after_ns.as_secs(),
            accepted_history_source_code: accepted_history_source_code(accepted_history_source),
        }
        .publish(env),
        KernelCircuitBreakerEvent::Tripped {
            breaker_id,
            tripped_at_ns,
            price_update:
                Observation {
                    price,
                    observed_at_ns,
                },
            is_enforced,
        } => CircuitBreakerTripped {
            asset: asset.clone(),
            breaker_id,
            tripped_at_secs: tripped_at_ns.as_secs(),
            price: i128::from(price.price),
            timestamp: observed_at_ns.as_secs(),
            is_enforced,
        }
        .publish(env),
    }
}
