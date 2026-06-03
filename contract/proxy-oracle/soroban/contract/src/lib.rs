#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, BytesN, Env, Vec};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{
    CircuitBreakerEvent as KernelCircuitBreakerEvent, CircuitBreakerSet, CircuitBreakerSetConfig,
};
use templar_proxy_oracle_soroban_common::{extend_instance_ttl, is_zero_wasm_hash};
pub use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, ContractError, MonotonicRunConfig as SorobanMonotonicRunConfig,
    NormalizedPrice, PriceData, PriceFeedClient, PriceFeedTrait, ProxyConfig, ProxyOracleClient,
    ProxyOracleTrait, RearmConfig, SetEnforcedConfig, SourceConfig,
    StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig, MAX_MANUAL_TRIP_METADATA_LEN,
};

pub type SorobanRearmConfig = RearmConfig;
pub type SorobanSetEnforcedConfig = SetEnforcedConfig;

mod codes;
mod conversion;
mod events;
mod refresh;
mod storage;

pub use events::{
    CacheBlocked, CircuitBreakerAdded, CircuitBreakerConfigSet, CircuitBreakerEnforcementSet,
    CircuitBreakerRearmed, CircuitBreakerRemoved, CircuitBreakerTripped, ContractUpgraded,
    ManualTripSet, ProxyRemoved, ProxySet, RefreshFailure, RefreshSuccess, TtlExtended,
};

use codes::breaker_error;
use conversion::{accepted_history_source, circuit_breaker_from_config, validate_proxy_config};
use refresh::{cached_accepted_no_older_than, refresh_one};
use storage::{
    add_asset, extend_persistent_ttl, get_assets, invalidate_cache, load_breakers, remove_asset,
    require_proxy_exists, store_breakers, DataKey,
};

pub(crate) const MAX_HISTORY_RECORDS: u32 = 32;
const MAX_SOURCES_PER_PROXY: u32 = 16;
const MAX_BREAKERS_PER_PROXY: usize = 16;

// `RefreshFailure` / `CacheBlocked` event codes published as the `code` field.
pub(crate) const STORAGE_FAILED_CODE: u32 = 3;
pub(crate) const SOURCE_UNAVAILABLE_CODE: u32 = 5;
pub(crate) const UNKNOWN_ASSET_CODE: u32 = 6;

#[contract]
pub struct SorobanProxyOracle;

#[derive(Clone)]
#[contracttype]
pub enum CachedStatus {
    Accepted(NormalizedPrice),
    Blocked(u32),
    ResolveFailed(u32),
}

#[derive(Clone)]
#[contracttype]
pub struct CachedProxyPrice {
    pub updated_at: u64,
    pub status: CachedStatus,
}

#[derive(Clone)]
#[contracttype]
pub struct CircuitBreakerSetView {
    pub breaker_count: u32,
    pub next_id: u32,
    pub is_manually_tripped: bool,
    pub is_blocking: bool,
}

#[derive(Clone)]
#[contracttype]
pub enum RefreshStatus {
    Accepted(NormalizedPrice),
    Blocked(u32),
    ResolveFailed(u32),
    UnknownAsset,
    SourceUnavailable,
}

/// Shared scaffolding for every owner-driven breaker mutation: auths via
/// `#[only_owner]` on the caller, loads the set, runs `op`, persists,
/// publishes the kernel-emitted events, invalidates the cache, and returns
/// whatever `op` returned. The auth check is done by the `#[only_owner]`
/// macro on the entrypoint, not here.
fn with_breakers<T>(
    env: &Env,
    asset: &Asset,
    op: impl FnOnce(
        &mut CircuitBreakerSet,
    ) -> Result<(T, AllocVec<KernelCircuitBreakerEvent>), ContractError>,
) -> Result<T, ContractError> {
    extend_instance_ttl(env);
    require_proxy_exists(env, asset)?;
    let mut breakers = load_breakers(env, asset)?;
    let (result, events) = op(&mut breakers)?;
    store_breakers(env, asset, &breakers)?;
    events::publish_breaker_events(env, asset, events);
    invalidate_cache(env, asset);
    Ok(result)
}

#[contractimpl]
impl SorobanProxyOracle {
    pub fn __constructor(env: Env, governance: Address, base: Asset) {
        extend_instance_ttl(&env);
        env.storage().instance().set(&DataKey::Base, &base);
        env.storage()
            .persistent()
            .set(&DataKey::Assets, &Vec::<Asset>::new(&env));
        set_owner(&env, &governance);
    }

    /// Owner-only runtime upgrade. Takes an already-uploaded WASM hash;
    /// does not accept a `migrate` payload to avoid widening the owner's
    /// authority surface beyond a typed code swap.
    #[only_owner]
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        if is_zero_wasm_hash(&new_wasm_hash) {
            return Err(ContractError::InvalidInput);
        }
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        ContractUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_proxy(env: Env, asset: Asset, config: ProxyConfig) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        validate_proxy_config(&config)?;
        env.storage()
            .persistent()
            .set(&DataKey::Proxy(asset.clone()), &config);
        add_asset(&env, &asset);
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Breakers(asset.clone()))
        {
            store_breakers(&env, &asset, &CircuitBreakerSet::empty())?;
        }
        invalidate_cache(&env, &asset);
        ProxySet {
            asset,
            source_count: config.sources.len(),
            min_sources: config.min_sources,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn remove_proxy(env: Env, asset: Asset) {
        extend_instance_ttl(&env);
        let storage = env.storage().persistent();
        storage.remove(&DataKey::Proxy(asset.clone()));
        storage.remove(&DataKey::Breakers(asset.clone()));
        storage.remove(&DataKey::History(asset.clone()));
        remove_asset(&env, &asset);
        invalidate_cache(&env, &asset);
        ProxyRemoved { asset }.publish(&env);
    }

    #[only_owner]
    pub fn configure_breakers(
        env: Env,
        asset: Asset,
        sample_interval_secs: u64,
        history_len: u32,
    ) -> Result<(), ContractError> {
        if history_len == 0 || history_len > MAX_HISTORY_RECORDS {
            return Err(ContractError::InvalidInput);
        }
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers.set_config(CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::from_secs(sample_interval_secs),
                history_len,
            });
            Ok(((), outcome.events))
        })
    }

    #[only_owner]
    pub fn add_breaker(
        env: Env,
        asset: Asset,
        breaker: CircuitBreakerConfig,
    ) -> Result<u32, ContractError> {
        let breaker = circuit_breaker_from_config(breaker)?;
        with_breakers(&env, &asset, |breakers| {
            if breakers.breaker_count() >= MAX_BREAKERS_PER_PROXY {
                return Err(ContractError::TooManyBreakers);
            }
            let breaker_id = breakers.next_id();
            let outcome = breakers.add(breaker_id, breaker).map_err(breaker_error)?;
            Ok((breaker_id, outcome.events))
        })
    }

    #[only_owner]
    pub fn remove_breaker(env: Env, asset: Asset, breaker_id: u32) -> Result<(), ContractError> {
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers.remove(breaker_id).map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
    }

    #[only_owner]
    pub fn rearm(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        config: RearmConfig,
    ) -> Result<(), ContractError> {
        let armed_after_ns = Nanoseconds::from_secs(config.armed_after_secs);
        let history_source = accepted_history_source(config.accepted_history_source_code)?;
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .rearm(breaker_id, armed_after_ns, history_source)
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
    }

    #[only_owner]
    pub fn set_enforced(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        config: SetEnforcedConfig,
    ) -> Result<(), ContractError> {
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .set_enforced(breaker_id, config.is_enforced)
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
    }

    #[only_owner]
    pub fn set_manual_trip(
        env: Env,
        actor: Address,
        asset: Asset,
        is_manually_tripped: bool,
        metadata: Option<Bytes>,
    ) -> Result<(), ContractError> {
        if metadata
            .as_ref()
            .is_some_and(|m| m.len() as usize > MAX_MANUAL_TRIP_METADATA_LEN)
        {
            return Err(ContractError::InvalidInput);
        }
        let kernel_metadata = metadata.as_ref().map(Bytes::to_alloc_vec);
        let metadata_for_event = metadata.clone();
        with_breakers(&env, &asset, |breakers| {
            use templar_proxy_oracle_kernel::primitive::AccountId as KernelAccountId;
            let outcome = breakers.set_manual_trip(
                is_manually_tripped,
                KernelAccountId::from_bytes([0_u8; 64]),
                kernel_metadata,
            );
            Ok(((), outcome.events))
        })?;
        ManualTripSet {
            asset,
            actor,
            is_manually_tripped,
            metadata: metadata_for_event,
        }
        .publish(&env);
        Ok(())
    }

    pub fn refresh(env: Env, assets: Vec<Asset>) -> Vec<(Asset, RefreshStatus)> {
        extend_instance_ttl(&env);
        let targets = if assets.is_empty() {
            get_assets(&env)
        } else {
            assets
        };
        let mut seen = Vec::new(&env);
        let mut results = Vec::new(&env);
        for asset in targets.iter() {
            if seen.iter().any(|entry| entry == asset) {
                continue;
            }
            seen.push_back(asset.clone());
            let status = refresh_one(&env, asset.clone());
            results.push_back((asset, status));
        }
        results
    }

    pub fn get_proxy(env: Env, asset: Asset) -> Option<ProxyConfig> {
        env.storage().persistent().get(&DataKey::Proxy(asset))
    }

    pub fn get_cached(env: Env, asset: Asset) -> Option<CachedProxyPrice> {
        env.storage().persistent().get(&DataKey::Cache(asset))
    }

    pub fn get_breaker_set_view(env: Env, asset: Asset) -> Option<CircuitBreakerSetView> {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Proxy(asset.clone()))
        {
            return None;
        }
        let breakers = load_breakers(&env, &asset).ok()?;
        Some(CircuitBreakerSetView {
            breaker_count: u32::try_from(breakers.breaker_count()).ok()?,
            next_id: breakers.next_id(),
            is_manually_tripped: breakers.is_manually_tripped(),
            is_blocking: breakers.is_blocking(),
        })
    }

    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
        extend_persistent_ttl(&env, &DataKey::Assets);
        let assets = get_assets(&env);
        for asset in assets.iter() {
            extend_persistent_ttl(&env, &DataKey::Proxy(asset.clone()));
            extend_persistent_ttl(&env, &DataKey::Breakers(asset.clone()));
            extend_persistent_ttl(&env, &DataKey::Cache(asset.clone()));
            extend_persistent_ttl(&env, &DataKey::History(asset));
        }
        TtlExtended {
            asset_count: assets.len(),
        }
        .publish(&env);
    }
}

/// Owner/governance surface is delegated to `stellar_access::ownable`, which
/// exposes `get_owner`, two-step `transfer_ownership`/`accept_ownership`, and
/// `renounce_ownership` via the standard `Ownable` trait. We re-export those
/// methods on the contract's client by exposing the trait's default
/// implementations through `#[contractimpl(contracttrait)]`.
#[contractimpl(contracttrait)]
impl Ownable for SorobanProxyOracle {}

/// Read API for `Sep40Adapter` contracts. The proxy oracle does not
/// implement SEP-40; adapters scale `NormalizedPrice` to their own
/// per-adapter decimals + resolution + base.
#[contractimpl]
impl ProxyOracleTrait for SorobanProxyOracle {
    fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice> {
        let cached = env
            .storage()
            .persistent()
            .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))?;
        let proxy_config = env
            .storage()
            .persistent()
            .get::<_, ProxyConfig>(&DataKey::Proxy(asset))?;
        let max_age = proxy_config.max_age_secs.unwrap_or(u64::MAX);
        cached_accepted_no_older_than(&cached, max_age, env.ledger().timestamp())
    }

    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>> {
        if records == 0 {
            return None;
        }
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<NormalizedPrice>>(&DataKey::History(asset))?;
        if history.is_empty() {
            return None;
        }
        let start = history.len().saturating_sub(records);
        Some(history.slice(start..))
    }
}

/// Admin / introspection helpers — deliberately named to avoid collision with
/// SEP-40's `base()` / `assets()`, since these mean different things here.
#[contractimpl]
impl SorobanProxyOracle {
    /// The base asset every source must report against (`source.base()`
    /// must match this on refresh). Not the same concept as SEP-40 `base`,
    /// which is per-adapter and describes what the adapter publishes.
    pub fn source_base(env: Env) -> Option<Asset> {
        env.storage().instance().get(&DataKey::Base)
    }

    /// Assets with a registered proxy config. Used by off-chain indexers
    /// and adapter deployer tooling.
    pub fn registered_assets(env: Env) -> Vec<Asset> {
        get_assets(&env)
    }
}

#[cfg(test)]
mod tests;
