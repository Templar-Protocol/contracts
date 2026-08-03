//! Builders for the proxy-oracle `admin_*` target calls: the shared serialization used by both the
//! manager CLI (building current-format proposals) and the legacy → generic mapping. Each returns a
//! [`FunctionCall`] carrying the method's JSON args and a per-method default gas (overridable).

use near_sdk::{
    json_types::{Base64VecU8, U128},
    near, Gas,
};
use templar_common::{oracle::pyth::PriceIdentifier, upgrade::UpgradeSource, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig},
    Proxy,
};
use templar_proxy_oracle_near_common::input::Source;

use crate::{FunctionCall, GAS_FOR_ADMIN_UPGRADE};

/// Default gas for the proxy/circuit-breaker admin calls (a single feed's storage + cache work).
pub const GAS_FOR_TARGET_DEFAULT: Gas = Gas::from_tgas(30);

/// Arg structs mirroring each `admin_*` method signature, so a call serializes to exactly the JSON
/// the proxy oracle expects.
#[near(serializers = [json])]
struct SetProxyArgs {
    id: PriceIdentifier,
    proxy: Option<Proxy<Source>>,
}
#[near(serializers = [json])]
struct ConfigureCircuitBreakersArgs {
    id: PriceIdentifier,
    config: CircuitBreakerSetConfig,
}
#[near(serializers = [json])]
struct AddCircuitBreakerArgs {
    id: PriceIdentifier,
    breaker_id: u32,
    breaker: CircuitBreaker,
}
#[near(serializers = [json])]
struct RemoveCircuitBreakerArgs {
    id: PriceIdentifier,
    breaker_id: u32,
}
#[near(serializers = [json])]
struct SetManualTripArgs {
    id: PriceIdentifier,
    is_manually_tripped: bool,
    metadata: Option<Base64VecU8>,
}
#[near(serializers = [json])]
struct RearmArgs {
    id: PriceIdentifier,
    breaker_id: u32,
    armed_after_ns: Nanoseconds,
    accepted_history_source: AcceptedHistorySource,
}
#[near(serializers = [json])]
struct SetEnforcedArgs {
    id: PriceIdentifier,
    breaker_id: u32,
    is_enforced: bool,
}
#[near(serializers = [json])]
struct AdminUpgradeArgs {
    code: UpgradeSource,
    migrate_args: Base64VecU8,
}

fn call<A: near_sdk::serde::Serialize>(
    method: &str,
    args: &A,
    gas: Gas,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    Ok(FunctionCall {
        method_name: method.to_owned(),
        args: Base64VecU8(near_sdk::serde_json::to_vec(args)?),
        attached_deposit: U128(0),
        gas,
    })
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_set_proxy(
    id: PriceIdentifier,
    proxy: Option<Proxy<Source>>,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_set_proxy",
        &SetProxyArgs { id, proxy },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_configure_circuit_breakers(
    id: PriceIdentifier,
    config: CircuitBreakerSetConfig,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_configure_circuit_breakers",
        &ConfigureCircuitBreakersArgs { id, config },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_add_circuit_breaker(
    id: PriceIdentifier,
    breaker_id: u32,
    breaker: CircuitBreaker,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_add_circuit_breaker",
        &AddCircuitBreakerArgs {
            id,
            breaker_id,
            breaker,
        },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_remove_circuit_breaker(
    id: PriceIdentifier,
    breaker_id: u32,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_remove_circuit_breaker",
        &RemoveCircuitBreakerArgs { id, breaker_id },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_set_manual_trip(
    id: PriceIdentifier,
    is_manually_tripped: bool,
    metadata: Option<Vec<u8>>,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_set_manual_trip",
        &SetManualTripArgs {
            id,
            is_manually_tripped,
            metadata: metadata.map(Base64VecU8),
        },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_rearm(
    id: PriceIdentifier,
    breaker_id: u32,
    armed_after_ns: Nanoseconds,
    accepted_history_source: AcceptedHistorySource,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_rearm",
        &RearmArgs {
            id,
            breaker_id,
            armed_after_ns,
            accepted_history_source,
        },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_set_enforced(
    id: PriceIdentifier,
    breaker_id: u32,
    is_enforced: bool,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_set_enforced",
        &SetEnforcedArgs {
            id,
            breaker_id,
            is_enforced,
        },
        gas.unwrap_or(GAS_FOR_TARGET_DEFAULT),
    )
}

/// Upgrade the proxy oracle. Defaults to 280 Tgas (a full self-deploy + migrate) when `gas` is `None`.
///
/// # Errors
///
/// If serializing the call args to JSON fails.
pub fn admin_upgrade(
    code: UpgradeSource,
    migrate_args: Base64VecU8,
    gas: Option<Gas>,
) -> Result<FunctionCall, near_sdk::serde_json::Error> {
    call(
        "admin_upgrade",
        &AdminUpgradeArgs { code, migrate_args },
        gas.unwrap_or(GAS_FOR_ADMIN_UPGRADE),
    )
}
