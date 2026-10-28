#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, BytesN, Env, Vec};
use stellar_access::ownable::{get_owner, set_owner, Ownable};
use stellar_macros::only_owner;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::Aggregator,
        circuit_breaker::{CircuitBreakerEvent as KernelCircuitBreakerEvent, CircuitBreakerSet},
    },
    Price,
};
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, is_zero_wasm_hash, parse_breaker_set_config, parse_proxy_config,
};
pub use templar_proxy_oracle_soroban_common::{
    AggregatorConfig, Asset, CircuitBreakerConfig, ContractError,
    CumulativeChangeConfig as SorobanCumulativeChangeConfig, MedianConfig,
    MonotonicRunConfig as SorobanMonotonicRunConfig, NormalizedPrice, PriceData, PriceFeedClient,
    PriceFeedTrait, ProxyConfig, ProxyOracleClient, ProxyOracleTrait, RearmConfig,
    SetEnforcedConfig, SorobanDecimal, SourceConfig,
    StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig, MAX_HISTORY_RECORDS,
    MAX_MANUAL_TRIP_METADATA_LEN,
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
    ManualTripSet, ProxyRemoved, ProxySet, RefreshEvaluated, RefreshFailure, RefreshSuccess,
    TtlExtended,
};

use codes::breaker_error;
use refresh::{cached_accepted_no_older_than, refresh_one};
use storage::{
    add_asset, clear_history, extend_persistent_ttl, invalidate_cache, load_assets, load_breakers,
    remove_asset, require_proxy_exists, store_breakers, DataKey,
};

const MAX_BREAKERS_PER_PROXY: usize = 16;
pub(crate) const MAX_REGISTERED_ASSETS: u32 = 64;

// `RefreshFailure` / `CacheBlocked` event codes published as the `code` field.
pub(crate) const AGGREGATION_FAILED_CODE: u32 = 1;
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
    pub sample_interval_secs: u64,
    pub history_len: u32,
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

fn with_breakers<T>(
    env: &Env,
    asset: &Asset,
    op: impl FnOnce(
        &mut CircuitBreakerSet,
    ) -> Result<(T, AllocVec<KernelCircuitBreakerEvent>), ContractError>,
) -> Result<(T, bool), ContractError> {
    extend_instance_ttl(env);
    require_proxy_exists(env, asset)?;
    let mut breakers = load_breakers(env, asset)?;
    let before = breakers.clone();
    let (result, events) = op(&mut breakers)?;
    if breakers == before {
        return Ok((result, false));
    }
    store_breakers(env, asset, &breakers)?;
    events::publish_breaker_events(env, asset, events);
    invalidate_cache(env, asset);
    Ok((result, true))
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

    /// Owner-only runtime upgrade. Signature matches the OpenZeppelin
    /// `stellar-contract-utils::upgradeable::Upgradeable` trait's
    /// `fn upgrade(env, new_wasm_hash, operator)`. The contract isn't
    /// pulled in directly (see "Known Limits" in the soroban README for
    /// the toolchain × sdk version blocker), but matching the shape now
    /// means migrating to the trait later is a no-op at the ABI level.
    /// Does not accept a `migrate` payload — avoids widening the owner's
    /// authority surface beyond a typed code swap.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        operator.require_auth();
        if get_owner(&env).as_ref() != Some(&operator) {
            return Err(ContractError::Unauthorized);
        }
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
        let proxy = parse_proxy_config(&config)?;
        for source in proxy.sources() {
            let client = PriceFeedClient::new(&env, &source.oracle);
            let decimals = client
                .try_decimals()
                .map_err(|_| ContractError::InvalidInput)?
                .map_err(|_| ContractError::InvalidInput)?;
            if decimals != source.expected_decimals {
                return Err(ContractError::InvalidInput);
            }
        }
        let storage = env.storage().persistent();
        let previous = storage.get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()));
        if previous.as_ref() == Some(&config) {
            return Ok(());
        }
        let aggregation_changed = previous
            .as_ref()
            .is_some_and(|current| current.aggregator != config.aggregator);
        if aggregation_changed {
            let mut breakers = load_breakers(&env, &asset)?;
            breakers.reset_preserving_next_id();
            store_breakers(&env, &asset, &breakers)?;
        } else if previous.is_none() && !storage.has(&DataKey::Breakers(asset.clone())) {
            store_breakers(&env, &asset, &CircuitBreakerSet::empty())?;
        }
        add_asset(&env, &asset)?;
        storage.set(&DataKey::Proxy(asset.clone()), &config);
        invalidate_cache(&env, &asset);
        clear_history(&env, &asset);
        let min_sources = match &proxy.aggregator {
            Aggregator::MedianLow(median) => median.min_sources,
            Aggregator::MedianHigh(median) => median.min_sources,
            Aggregator::Priority(_) => 1,
        };
        ProxySet {
            asset,
            source_count: u32::try_from(proxy.sources().len())
                .map_err(|_| ContractError::InvalidInput)?,
            min_sources,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn remove_proxy(env: Env, asset: Asset) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        let storage = env.storage().persistent();
        storage.remove(&DataKey::Proxy(asset.clone()));
        storage.remove(&DataKey::Breakers(asset.clone()));
        storage.remove(&DataKey::History(asset.clone()));
        remove_asset(&env, &asset)?;
        invalidate_cache(&env, &asset);
        ProxyRemoved { asset }.publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn configure_breakers(
        env: Env,
        asset: Asset,
        sample_interval_secs: u64,
        history_len: u32,
    ) -> Result<(), ContractError> {
        let config = parse_breaker_set_config(sample_interval_secs, history_len)?;
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers.set_config(config).map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn add_breaker(
        env: Env,
        asset: Asset,
        breaker: CircuitBreakerConfig,
    ) -> Result<u32, ContractError> {
        let baseline = match &breaker {
            CircuitBreakerConfig::CumulativeChange(_) => {
                let proxy_config = env
                    .storage()
                    .persistent()
                    .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
                    .ok_or(ContractError::InvalidInput)?;
                env.storage()
                    .persistent()
                    .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))
                    .and_then(|cached| {
                        cached_accepted_no_older_than(
                            &cached,
                            proxy_config.max_age_secs,
                            env.ledger().timestamp(),
                        )
                    })
                    .filter(|price| price.mantissa > 0)
                    .map(|price| Price {
                        price: price.mantissa,
                        conf: 0,
                        expo: price.expo,
                        publish_time_ns: Nanoseconds::from_secs(price.timestamp),
                    })
            }
            CircuitBreakerConfig::StepwiseChange(_)
            | CircuitBreakerConfig::MonotonicRun(_)
            | CircuitBreakerConfig::WindowedChangeDelta(_) => None,
        };
        let breaker = breaker.to_kernel(baseline)?;
        with_breakers(&env, &asset, |breakers| {
            if breakers.breaker_count() >= MAX_BREAKERS_PER_PROXY {
                return Err(ContractError::TooManyBreakers);
            }
            let breaker_id = breakers.next_id();
            let outcome = breakers.add(breaker_id, breaker).map_err(breaker_error)?;
            Ok((breaker_id, outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn remove_breaker(env: Env, asset: Asset, breaker_id: u32) -> Result<(), ContractError> {
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers.remove(breaker_id).map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn rearm(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        config: RearmConfig,
    ) -> Result<(), ContractError> {
        let armed_after_secs = env
            .ledger()
            .timestamp()
            .checked_add(config.arming_delay_secs)
            .ok_or(ContractError::InvalidInput)?;
        let armed_after_ns =
            Nanoseconds::checked_from_secs(armed_after_secs).ok_or(ContractError::InvalidInput)?;
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .rearm(breaker_id, armed_after_ns)
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
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
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn set_manual_trip(
        env: Env,
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
        let ((), did_mutate) = with_breakers(&env, &asset, |breakers| {
            use templar_proxy_oracle_kernel::primitive::AccountId as KernelAccountId;
            let outcome = breakers.set_manual_trip(
                is_manually_tripped,
                KernelAccountId::from_bytes([0_u8; 64]),
                kernel_metadata,
            );
            Ok(((), outcome.events))
        })?;
        if !did_mutate {
            return Ok(());
        }
        ManualTripSet {
            asset,
            is_manually_tripped,
            metadata,
        }
        .publish(&env);
        Ok(())
    }

    pub fn refresh(env: Env, asset: Asset) -> RefreshStatus {
        extend_instance_ttl(&env);
        refresh_one(&env, asset)
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
            sample_interval_secs: breakers.sample_interval_ns().as_secs(),
            history_len: breakers.accepted_history().capacity(),
            is_manually_tripped: breakers.is_manually_tripped(),
            is_blocking: breakers.is_blocking(),
        })
    }

    pub fn extend_ttl(env: Env, asset: Asset) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_proxy_exists(&env, &asset)?;
        extend_persistent_ttl(&env, &DataKey::Assets);
        extend_persistent_ttl(&env, &DataKey::Proxy(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::Breakers(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::Cache(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::History(asset.clone()));
        TtlExtended { asset }.publish(&env);
        Ok(())
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
        cached_accepted_no_older_than(&cached, proxy_config.max_age_secs, env.ledger().timestamp())
    }

    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>> {
        if records == 0 {
            return None;
        }
        let breakers = load_breakers(&env, &asset).ok()?;
        if breakers.is_blocking() {
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

    fn source_base(env: Env) -> Option<Asset> {
        env.storage().instance().get(&DataKey::Base)
    }
}

/// Admin / introspection helpers — deliberately named to avoid collision with
/// SEP-40's `base()` / `assets()`, since these mean different things here.
#[contractimpl]
impl SorobanProxyOracle {
    /// Assets with a registered proxy config. Used by off-chain indexers
    /// and adapter deployer tooling.
    pub fn registered_assets(env: Env) -> Result<Vec<Asset>, ContractError> {
        load_assets(&env)
    }
}

#[cfg(test)]
mod tests;
