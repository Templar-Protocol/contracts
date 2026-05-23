#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
// This is a Soroban ABI requirement; taking them by reference is not valid.
// The lint is suppressed at the file level because every public method in
// the #[contractimpl] blocks is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, Address, Bytes, Env, Vec,
};
use templar_primitives::{Decimal, Nanoseconds};
use templar_proxy_oracle_kernel::{
    primitive::AccountId as KernelAccountId,
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            AcceptedHistorySource, CircuitBreaker, CircuitBreakerError,
            CircuitBreakerEvent as KernelCircuitBreakerEvent, CircuitBreakerSet,
            CircuitBreakerSetConfig, CircuitBreakerUpdate, MonotonicRun, Observation,
            PriceBlockedReason, StepwiseChange, WindowedChangeDelta,
        },
        FreshnessFilter, Proxy, WeightedSource,
    },
    Price,
};
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
};
pub use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, CircuitBreakerUpdateConfig, ContractError,
    MonotonicRunConfig as SorobanMonotonicRunConfig, PriceData, ProxyConfig,
    RearmConfig as SorobanRearmConfig, Role, SetEnforcedConfig as SorobanSetEnforcedConfig,
    SourceConfig, StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig, MAX_MANUAL_TRIP_METADATA_LEN,
};

const MAX_HISTORY_RECORDS: u32 = 32;
const MAX_SOURCES_PER_PROXY: u32 = 16;
const MAX_BREAKERS_PER_PROXY: usize = 16;
const RESOLVE_FAILED_STORAGE_CODE: u32 = 3;
const CONVERSION_FAILED_CODE: u32 = 4;
const SOURCE_UNAVAILABLE_CODE: u32 = 5;

#[contract]
pub struct SorobanProxyOracle;

pub mod events;
pub use events::*;

#[contractclient(name = "PriceFeedClient")]
pub trait PriceFeedTrait {
    fn base(env: Env) -> Asset;
    fn assets(env: Env) -> Vec<Asset>;
    fn decimals(env: Env) -> u32;
    fn resolution(env: Env) -> u32;
    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData>;
    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>>;
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}

#[derive(Clone)]
#[contracttype]
pub enum CachedStatus {
    Accepted(PriceData),
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
    Accepted(PriceData),
    Blocked(u32),
    ResolveFailed(u32),
    UnknownAsset,
    SourceUnavailable,
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Governance,
    Base,
    Decimals,
    Resolution,
    Assets,
    Proxy(Asset),
    Breakers(Asset),
    Cache(Asset),
    History(Asset),
    Role(Role, Address),
    RoleAccounts(Role),
}

fn require_governance(env: &Env) -> Result<Address, ContractError> {
    let governance: Address = env
        .storage()
        .instance()
        .get(&DataKey::Governance)
        .ok_or(ContractError::MissingConfig)?;
    governance.require_auth();
    Ok(governance)
}

fn get_assets(env: &Env) -> Vec<Asset> {
    env.storage()
        .persistent()
        .get(&DataKey::Assets)
        .unwrap_or_else(|| Vec::new(env))
}

fn add_asset(env: &Env, asset: &Asset) {
    let mut assets = get_assets(env);
    if !assets.iter().any(|entry| &entry == asset) {
        assets.push_back(asset.clone());
        env.storage().persistent().set(&DataKey::Assets, &assets);
    }
}

fn remove_asset(env: &Env, asset: &Asset) {
    let mut assets = get_assets(env);
    if let Some(index) = assets
        .iter()
        .position(|entry| &entry == asset)
        .and_then(|i| u32::try_from(i).ok())
    {
        assets.remove(index);
        env.storage().persistent().set(&DataKey::Assets, &assets);
    }
}

fn invalidate_cache(env: &Env, asset: &Asset) {
    env.storage()
        .persistent()
        .remove(&DataKey::Cache(asset.clone()));
}

fn load_breakers(env: &Env, asset: &Asset) -> Result<CircuitBreakerSet, ContractError> {
    let Some(bytes) = env
        .storage()
        .persistent()
        .get::<_, Bytes>(&DataKey::Breakers(asset.clone()))
    else {
        return Ok(CircuitBreakerSet::empty());
    };
    postcard::from_bytes(&bytes.to_alloc_vec()).map_err(|_| ContractError::StorageError)
}

fn store_breakers(
    env: &Env,
    asset: &Asset,
    breakers: &CircuitBreakerSet,
) -> Result<(), ContractError> {
    let bytes = postcard::to_allocvec(breakers).map_err(|_| ContractError::StorageError)?;
    env.storage().persistent().set(
        &DataKey::Breakers(asset.clone()),
        &Bytes::from_slice(env, &bytes),
    );
    Ok(())
}

fn require_proxy_exists(env: &Env, asset: &Asset) -> Result<(), ContractError> {
    if env
        .storage()
        .persistent()
        .has(&DataKey::Proxy(asset.clone()))
    {
        Ok(())
    } else {
        Err(ContractError::InvalidInput)
    }
}

fn require_role(env: &Env, account: &Address, role: Role) -> Result<(), ContractError> {
    if env
        .storage()
        .persistent()
        .get::<_, bool>(&DataKey::Role(role, account.clone()))
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(ContractError::Unauthorized)
    }
}

fn role_accounts(env: &Env, role: Role) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::RoleAccounts(role))
        .unwrap_or_else(|| Vec::new(env))
}

fn set_role_account(env: &Env, account: &Address, role: Role, is_granted: bool) {
    let mut accounts = role_accounts(env, role.clone());
    let position = accounts.iter().position(|entry| &entry == account);
    if is_granted {
        env.storage()
            .persistent()
            .set(&DataKey::Role(role.clone(), account.clone()), &true);
        if position.is_none() {
            accounts.push_back(account.clone());
        }
    } else {
        env.storage()
            .persistent()
            .remove(&DataKey::Role(role.clone(), account.clone()));
        if let Some(index) = position.and_then(|i| u32::try_from(i).ok()) {
            accounts.remove(index);
        }
    }
    env.storage()
        .persistent()
        .set(&DataKey::RoleAccounts(role), &accounts);
}

fn source_price_to_kernel(
    source_price: PriceData,
    source_decimals: u32,
) -> Result<Price, ContractError> {
    let mut value = source_price.price;
    let mut expo = i32::try_from(source_decimals)
        .map_err(|_| ContractError::ConversionOverflow)?
        .checked_neg()
        .ok_or(ContractError::ConversionOverflow)?;
    while value > i128::from(i64::MAX) || value < i128::from(i64::MIN) {
        value /= 10;
        expo = expo
            .checked_add(1)
            .ok_or(ContractError::ConversionOverflow)?;
    }
    Ok(Price {
        price: i64::try_from(value).map_err(|_| ContractError::ConversionOverflow)?,
        conf: 0,
        expo,
        publish_time_ns: Nanoseconds::from_secs(source_price.timestamp),
    })
}

fn kernel_price_to_sep40(price: Price, decimals: u32) -> Result<PriceData, ContractError> {
    let decimals = i32::try_from(decimals).map_err(|_| ContractError::ConversionOverflow)?;
    let scale = decimals
        .checked_add(price.expo)
        .ok_or(ContractError::ConversionOverflow)?;
    let mut value = i128::from(price.price);
    if scale >= 0 {
        value = value
            .checked_mul(
                10_i128
                    .checked_pow(scale.unsigned_abs())
                    .ok_or(ContractError::ConversionOverflow)?,
            )
            .ok_or(ContractError::ConversionOverflow)?;
    } else {
        value /= 10_i128
            .checked_pow(scale.unsigned_abs())
            .ok_or(ContractError::ConversionOverflow)?;
    }
    Ok(PriceData {
        price: value,
        timestamp: price.publish_time_ns.as_secs(),
    })
}

fn blocked_reason_code(reason: PriceBlockedReason) -> u32 {
    match reason {
        PriceBlockedReason::ManuallyTripped => 1,
        PriceBlockedReason::BreakerTripped { .. } => 2,
    }
}

fn breaker_kind_code(breaker: &CircuitBreaker) -> u32 {
    match breaker {
        CircuitBreaker::StepwiseChange(_) => 1,
        CircuitBreaker::MonotonicRun(_) => 2,
        CircuitBreaker::WindowedChangeDelta(_) => 3,
    }
}

fn accepted_history_source_code(source: AcceptedHistorySource) -> u32 {
    match source {
        AcceptedHistorySource::Empty => 0,
        AcceptedHistorySource::Observed => 1,
    }
}

fn publish_refresh_event(env: &Env, asset: &Asset, status: &RefreshStatus) {
    match status {
        RefreshStatus::Accepted(price) => RefreshSuccess {
            asset: asset.clone(),
            price: price.price,
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
            code: 6,
        }
        .publish(env),
    }
}

fn publish_manual_trip_event(
    env: &Env,
    asset: &Asset,
    actor: &Address,
    is_manually_tripped: bool,
    metadata: Option<Bytes>,
) {
    ManualTripSet {
        asset: asset.clone(),
        actor: actor.clone(),
        is_manually_tripped,
        metadata,
    }
    .publish(env);
}

fn publish_breaker_events(env: &Env, asset: &Asset, events: AllocVec<KernelCircuitBreakerEvent>) {
    for event in events {
        match event {
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
}

fn resolve_error_code(error: templar_proxy_oracle_kernel::proxy::ResolveError) -> u32 {
    match error {
        templar_proxy_oracle_kernel::proxy::ResolveError::Aggregation(_) => 1,
        templar_proxy_oracle_kernel::proxy::ResolveError::CircuitBreaker(_) => 2,
    }
}

fn breaker_error(error: CircuitBreakerError) -> ContractError {
    match error {
        CircuitBreakerError::TooManyBreakers => ContractError::TooManyBreakers,
        _ => ContractError::BreakerError,
    }
}

fn decimal_from_repr(repr: Vec<u64>) -> Result<Decimal, ContractError> {
    if repr.len() != 8 {
        return Err(ContractError::InvalidInput);
    }
    let mut raw = [0_u64; 8];
    for (index, value) in repr.iter().enumerate() {
        raw[index] = value;
    }
    Ok(Decimal::from_repr(raw))
}

fn accepted_history_source(value: u32) -> Result<AcceptedHistorySource, ContractError> {
    match value {
        0 => Ok(AcceptedHistorySource::Empty),
        1 => Ok(AcceptedHistorySource::Observed),
        _ => Err(ContractError::InvalidInput),
    }
}

fn circuit_breaker_from_config(
    config: CircuitBreakerConfig,
) -> Result<CircuitBreaker, ContractError> {
    match config {
        CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change_repr,
        }) => {
            let max_relative_change = decimal_from_repr(max_relative_change_repr)?;
            if max_relative_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change,
            }))
        }
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak,
            min_relative_step_change_repr,
        }) => {
            if max_streak == 0 {
                return Err(ContractError::InvalidInput);
            }
            let min_relative_step_change = decimal_from_repr(min_relative_step_change_repr)?;
            if min_relative_step_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak,
                min_relative_step_change,
            }))
        }
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len,
            lookback_windows,
            max_relative_change_delta_repr,
        }) => {
            if window_len < 2 {
                return Err(ContractError::InvalidInput);
            }
            if lookback_windows == 0 {
                return Err(ContractError::InvalidInput);
            }
            let max_relative_change_delta = decimal_from_repr(max_relative_change_delta_repr)?;
            if max_relative_change_delta.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                window_len,
                lookback_windows,
                max_relative_change_delta,
            }))
        }
    }
}

fn circuit_breaker_update_from_config(
    update: CircuitBreakerUpdateConfig,
) -> Result<CircuitBreakerUpdate, ContractError> {
    match update {
        CircuitBreakerUpdateConfig::SetEnforced(SorobanSetEnforcedConfig { is_enforced }) => {
            Ok(CircuitBreakerUpdate::SetEnforced { is_enforced })
        }
        CircuitBreakerUpdateConfig::Rearm(SorobanRearmConfig {
            armed_after_secs,
            accepted_history_source_code,
        }) => Ok(CircuitBreakerUpdate::Rearm {
            armed_after_ns: Nanoseconds::from_secs(armed_after_secs),
            accepted_history_source: accepted_history_source(accepted_history_source_code)?,
        }),
    }
}

fn kernel_proxy(config: &ProxyConfig) -> Proxy<u32> {
    let mut median =
        MedianLow::new((0..config.sources.len()).map(|index| WeightedSource::new(index, 1)));
    median.min_sources = config.min_sources;
    Proxy::new(
        Aggregator::MedianLow(median),
        FreshnessFilter::new(
            config.max_age_secs.map(Nanoseconds::from_secs),
            config.max_clock_drift_secs.map(Nanoseconds::from_secs),
        ),
    )
}

fn source_kernel_price(env: &Env, source: SourceConfig, expected_base: &Asset) -> Option<Price> {
    let client = PriceFeedClient::new(env, &source.oracle);
    let base = client.try_base().ok()?.ok()?;
    if &base != expected_base {
        return None;
    }
    let decimals = client.try_decimals().ok()?.ok()?;
    let price = client.try_lastprice(&source.asset).ok()?.ok()??;
    source_price_to_kernel(price, decimals).ok()
}

fn cache_failed_refresh(env: &Env, asset: Asset, now: Nanoseconds, code: u32) -> RefreshStatus {
    env.storage().persistent().set(
        &DataKey::Cache(asset.clone()),
        &CachedProxyPrice {
            updated_at: now.as_secs(),
            status: CachedStatus::ResolveFailed(code),
        },
    );
    let status = if code == SOURCE_UNAVAILABLE_CODE {
        RefreshStatus::SourceUnavailable
    } else {
        RefreshStatus::ResolveFailed(code)
    };
    publish_refresh_event(env, &asset, &status);
    status
}

fn cached_accepted_no_older_than(
    cached: &CachedProxyPrice,
    max_age_secs: u64,
    now: u64,
) -> Option<PriceData> {
    let CachedStatus::Accepted(price) = &cached.status else {
        return None;
    };
    if now >= price.timestamp && now.saturating_sub(price.timestamp) > max_age_secs {
        return None;
    }
    Some(price.clone())
}

fn push_history(env: &Env, asset: &Asset, price: &PriceData) {
    let key = DataKey::History(asset.clone());
    let mut history = env
        .storage()
        .persistent()
        .get::<_, Vec<PriceData>>(&key)
        .unwrap_or_else(|| Vec::new(env));
    for index in 0..history.len() {
        if history
            .get(index)
            .is_some_and(|entry| entry.timestamp == price.timestamp)
        {
            history.remove(index);
            break;
        }
    }
    history.push_back(price.clone());
    while history.len() > MAX_HISTORY_RECORDS {
        history.remove(0);
    }
    env.storage().persistent().set(&key, &history);
}

fn refresh_one(env: &Env, asset: Asset) -> RefreshStatus {
    let now = Nanoseconds::from_secs(env.ledger().timestamp());
    let Some(config) = env
        .storage()
        .persistent()
        .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
    else {
        let status = RefreshStatus::UnknownAsset;
        publish_refresh_event(env, &asset, &status);
        return status;
    };
    let Some(expected_base) = env.storage().instance().get::<_, Asset>(&DataKey::Base) else {
        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
    };
    let Some(decimals) = env.storage().instance().get::<_, u32>(&DataKey::Decimals) else {
        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
    };

    let mut prices = AllocVec::with_capacity(config.sources.len() as usize);
    for source in config.sources.iter() {
        prices.push(source_kernel_price(env, source, &expected_base));
    }
    if prices.iter().all(Option::is_none) {
        return cache_failed_refresh(env, asset, now, SOURCE_UNAVAILABLE_CODE);
    }

    let Ok(mut breakers) = load_breakers(env, &asset) else {
        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
    };
    let proxy = kernel_proxy(&config);
    let status = match proxy.resolve(&mut breakers, prices, now) {
        Ok(outcome) => {
            publish_breaker_events(env, &asset, outcome.events);
            match outcome.value {
                Ok(price) => match kernel_price_to_sep40(price, decimals) {
                    Ok(price) => {
                        if store_breakers(env, &asset, &breakers).is_err() {
                            return cache_failed_refresh(
                                env,
                                asset,
                                now,
                                RESOLVE_FAILED_STORAGE_CODE,
                            );
                        }
                        push_history(env, &asset, &price);
                        let cached = CachedProxyPrice {
                            updated_at: now.as_secs(),
                            status: CachedStatus::Accepted(price.clone()),
                        };
                        env.storage()
                            .persistent()
                            .set(&DataKey::Cache(asset.clone()), &cached);
                        let status = RefreshStatus::Accepted(price);
                        publish_refresh_event(env, &asset, &status);
                        return status;
                    }
                    Err(_) => CachedStatus::ResolveFailed(CONVERSION_FAILED_CODE),
                },
                Err(reason) => CachedStatus::Blocked(blocked_reason_code(reason)),
            }
        }
        Err(error) => CachedStatus::ResolveFailed(resolve_error_code(error)),
    };

    if store_breakers(env, &asset, &breakers).is_err() {
        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
    }
    let refresh_status = match &status {
        CachedStatus::Accepted(price) => RefreshStatus::Accepted(price.clone()),
        CachedStatus::Blocked(code) => RefreshStatus::Blocked(*code),
        CachedStatus::ResolveFailed(code) => RefreshStatus::ResolveFailed(*code),
    };
    publish_refresh_event(env, &asset, &refresh_status);
    env.storage().persistent().set(
        &DataKey::Cache(asset),
        &CachedProxyPrice {
            updated_at: now.as_secs(),
            status,
        },
    );
    refresh_status
}

#[contractimpl]
impl SorobanProxyOracle {
    pub fn __constructor(
        env: Env,
        governance: Address,
        base: Asset,
        decimals: u32,
        resolution: u32,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        if env.storage().instance().has(&DataKey::Governance) {
            return Err(ContractError::AlreadyInitialized);
        }
        if resolution == 0 {
            return Err(ContractError::InvalidInput);
        }
        if decimals > 18 {
            return Err(ContractError::InvalidInput);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::Base, &base);
        env.storage().instance().set(&DataKey::Decimals, &decimals);
        env.storage()
            .instance()
            .set(&DataKey::Resolution, &resolution);
        env.storage()
            .persistent()
            .set(&DataKey::Assets, &Vec::<Asset>::new(&env));
        Ok(())
    }

    pub fn set_governance(env: Env, new_governance: Address) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        let old_governance = require_governance(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        GovernanceHandoff {
            old_governance,
            new_governance,
        }
        .publish(&env);
        Ok(())
    }

    pub fn set_proxy(
        env: Env,
        asset: Asset,
        config: Option<ProxyConfig>,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        if let Some(config) = config {
            if config.sources.is_empty() || config.sources.len() > MAX_SOURCES_PER_PROXY {
                return Err(ContractError::TooManySources);
            }
            if config.min_sources == 0 || config.min_sources > config.sources.len() {
                return Err(ContractError::InvalidInput);
            }
            let sources_len = config.sources.len();
            for i in 0..sources_len {
                let src_i = config.sources.get(i).ok_or(ContractError::InvalidInput)?;
                for j in (i + 1)..sources_len {
                    let src_j = config.sources.get(j).ok_or(ContractError::InvalidInput)?;
                    if src_i.oracle == src_j.oracle && src_i.asset == src_j.asset {
                        return Err(ContractError::InvalidInput);
                    }
                }
            }
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
                asset: asset.clone(),
                source_count: config.sources.len(),
                min_sources: config.min_sources,
            }
            .publish(&env);
        } else {
            env.storage()
                .persistent()
                .remove(&DataKey::Proxy(asset.clone()));
            env.storage()
                .persistent()
                .remove(&DataKey::Breakers(asset.clone()));
            env.storage()
                .persistent()
                .remove(&DataKey::History(asset.clone()));
            remove_asset(&env, &asset);
            invalidate_cache(&env, &asset);
            ProxyRemoved {
                asset: asset.clone(),
            }
            .publish(&env);
        }
        Ok(())
    }

    pub fn configure_breakers(
        env: Env,
        asset: Asset,
        sample_interval_secs: u64,
        history_len: u32,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        if history_len == 0 || history_len > MAX_HISTORY_RECORDS {
            return Err(ContractError::InvalidInput);
        }
        let mut breakers = load_breakers(&env, &asset)?;
        let outcome = breakers.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::from_secs(sample_interval_secs),
            history_len,
        });
        store_breakers(&env, &asset, &breakers)?;
        publish_breaker_events(&env, &asset, outcome.events);
        invalidate_cache(&env, &asset);
        Ok(())
    }

    pub fn add_breaker(
        env: Env,
        asset: Asset,
        breaker: CircuitBreakerConfig,
    ) -> Result<u32, ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        let mut breakers = load_breakers(&env, &asset)?;
        if breakers.breaker_count() >= MAX_BREAKERS_PER_PROXY {
            return Err(ContractError::TooManyBreakers);
        }
        let breaker_id = breakers.next_id();
        let outcome = breakers
            .add(breaker_id, circuit_breaker_from_config(breaker)?)
            .map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
        publish_breaker_events(&env, &asset, outcome.events);
        invalidate_cache(&env, &asset);
        Ok(breaker_id)
    }

    pub fn remove_breaker(env: Env, asset: Asset, breaker_id: u32) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        let mut breakers = load_breakers(&env, &asset)?;
        let outcome = breakers.remove(breaker_id).map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
        publish_breaker_events(&env, &asset, outcome.events);
        invalidate_cache(&env, &asset);
        Ok(())
    }

    pub fn update_breaker(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        update: CircuitBreakerUpdateConfig,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        let mut breakers = load_breakers(&env, &asset)?;
        let outcome = breakers
            .update(breaker_id, circuit_breaker_update_from_config(update)?)
            .map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
        publish_breaker_events(&env, &asset, outcome.events);
        invalidate_cache(&env, &asset);
        Ok(())
    }

    pub fn set_circuit_breaker_role(
        env: Env,
        account: Address,
        role: Role,
        is_granted: bool,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        set_role_account(&env, &account, role.clone(), is_granted);
        CircuitBreakerRoleSet {
            account,
            role,
            is_granted,
        }
        .publish(&env);
        Ok(())
    }

    pub fn has_role(env: Env, account: Address, role: Role) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::Role(role, account))
            .unwrap_or(false)
    }

    pub fn set_manual_trip(
        env: Env,
        actor: Address,
        asset: Asset,
        is_manually_tripped: bool,
        metadata: Option<Bytes>,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        actor.require_auth();
        require_role(
            &env,
            &actor,
            if is_manually_tripped {
                Role::OfflineManualTrip
            } else {
                Role::OfflineManualUntrip
            },
        )?;
        require_proxy_exists(&env, &asset)?;
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.len() as usize > MAX_MANUAL_TRIP_METADATA_LEN)
        {
            return Err(ContractError::InvalidInput);
        }
        let mut breakers = load_breakers(&env, &asset)?;
        let kernel_metadata = metadata.as_ref().map(Bytes::to_alloc_vec);
        let outcome = breakers.set_manual_trip(
            is_manually_tripped,
            KernelAccountId::from_bytes([0_u8; 64]),
            kernel_metadata,
        );
        store_breakers(&env, &asset, &breakers)?;
        for event in outcome.events {
            if let KernelCircuitBreakerEvent::ManualTripSet {
                is_manually_tripped,
                ..
            } = event
            {
                publish_manual_trip_event(
                    &env,
                    &asset,
                    &actor,
                    is_manually_tripped,
                    metadata.clone(),
                );
            }
        }
        invalidate_cache(&env, &asset);
        Ok(())
    }

    pub fn refresh(env: Env, assets: Vec<Asset>) -> Vec<(Asset, RefreshStatus)> {
        extend_instance_ttl(&env);
        let targets = if assets.is_empty() {
            get_assets(&env)
        } else {
            assets
        };
        let mut results = Vec::new(&env);
        for asset in targets.iter() {
            let status = refresh_one(&env, asset.clone());
            results.push_back((asset, status));
        }
        results
    }

    pub fn get_proxy(env: Env, asset: Asset) -> Option<ProxyConfig> {
        env.storage()
            .persistent()
            .get(&DataKey::Proxy(asset.clone()))
    }

    pub fn get_cached(env: Env, asset: Asset) -> Option<CachedProxyPrice> {
        env.storage()
            .persistent()
            .get(&DataKey::Cache(asset.clone()))
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

    pub fn governance(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Governance)
    }

    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
        let storage = env.storage().persistent();
        let assets_key = DataKey::Assets;
        if storage.has(&assets_key) {
            storage.extend_ttl(&assets_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
        }
        let assets = get_assets(&env);
        for asset in assets.iter() {
            let proxy_key = DataKey::Proxy(asset.clone());
            if storage.has(&proxy_key) {
                storage.extend_ttl(&proxy_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
            }
            let breakers_key = DataKey::Breakers(asset.clone());
            if storage.has(&breakers_key) {
                storage.extend_ttl(&breakers_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
            }
            let cache_key = DataKey::Cache(asset.clone());
            if storage.has(&cache_key) {
                storage.extend_ttl(&cache_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
            }
            let history_key = DataKey::History(asset.clone());
            if storage.has(&history_key) {
                storage.extend_ttl(&history_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
            }
        }
        for role in [Role::OfflineManualTrip, Role::OfflineManualUntrip] {
            let role_accounts_key = DataKey::RoleAccounts(role.clone());
            if !storage.has(&role_accounts_key) {
                continue;
            }
            storage.extend_ttl(
                &role_accounts_key,
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            for account in role_accounts(&env, role.clone()).iter() {
                let role_key = DataKey::Role(role.clone(), account);
                if storage.has(&role_key) {
                    storage.extend_ttl(&role_key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
                }
            }
        }
        TtlExtended {
            asset_count: assets.len(),
        }
        .publish(&env);
    }
}

// SEP-40 getters cannot return Option or Result per the interface contract.
// Panic on missing key is the documented fail-closed behavior (Task 5).
#[allow(clippy::expect_used)]
#[contractimpl]
impl PriceFeedTrait for SorobanProxyOracle {
    fn base(env: Env) -> Asset {
        env.storage().instance().get(&DataKey::Base).expect("base")
    }

    fn assets(env: Env) -> Vec<Asset> {
        env.storage()
            .persistent()
            .get(&DataKey::Assets)
            .expect("assets")
    }

    fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .expect("decimals")
    }

    fn resolution(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Resolution)
            .expect("resolution")
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<PriceData>>(&DataKey::History(asset.clone()))?;
        let mut index = history.len();
        while index > 0 {
            index -= 1;
            let price = history.get(index)?;
            if price.timestamp == timestamp {
                return Some(price);
            }
        }
        None
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<PriceData>>(&DataKey::History(asset.clone()))?;
        if history.is_empty() {
            return None;
        }
        let start = history.len().saturating_sub(records);
        if records == 0 {
            return None;
        }
        Some(history.slice(start..))
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        let cached = env
            .storage()
            .persistent()
            .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))?;
        let proxy_config = env
            .storage()
            .persistent()
            .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))?;
        let max_age = proxy_config.max_age_secs.unwrap_or(u64::MAX);
        cached_accepted_no_older_than(&cached, max_age, env.ledger().timestamp())
    }
}

#[cfg(test)]
mod tests;
