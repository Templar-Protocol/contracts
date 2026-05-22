#![no_std]

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, symbol_short, Address, Bytes, Env,
    Symbol, Vec,
};
use templar_primitives::{Decimal, Nanoseconds};
use templar_proxy_oracle_kernel::{
    primitive::AccountId as KernelAccountId,
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            AcceptedHistorySource, CircuitBreaker, CircuitBreakerError, CircuitBreakerSet,
            CircuitBreakerSetConfig, CircuitBreakerUpdate, MonotonicRun, PriceBlockedReason,
            StepwiseChange, WindowedChangeDelta,
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
    RearmConfig as SorobanRearmConfig, SetEnforcedConfig as SorobanSetEnforcedConfig, SourceConfig,
    StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig,
};

const MAX_HISTORY_RECORDS: u32 = 32;
const MAX_SOURCES_PER_PROXY: u32 = 16;
const MAX_BREAKERS_PER_PROXY: usize = 16;
const RESOLVE_FAILED_STORAGE_CODE: u32 = 3;
const CONVERSION_FAILED_CODE: u32 = 4;
const SOURCE_UNAVAILABLE_CODE: u32 = 5;

#[contract]
pub struct SorobanProxyOracle;

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
    if let Some(index) = assets.iter().position(|entry| &entry == asset) {
        assets.remove(index as u32);
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
                    .checked_pow(scale as u32)
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
        }) => Ok(CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: decimal_from_repr(max_relative_change_repr)?,
        })),
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak,
            min_relative_step_change_repr,
        }) => Ok(CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak,
            min_relative_step_change: decimal_from_repr(min_relative_step_change_repr)?,
        })),
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len,
            lookback_windows,
            max_relative_change_delta_repr,
        }) => Ok(CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
            window_len,
            lookback_windows,
            max_relative_change_delta: decimal_from_repr(max_relative_change_delta_repr)?,
        })),
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
        &DataKey::Cache(asset),
        &CachedProxyPrice {
            updated_at: now.as_secs(),
            status: CachedStatus::ResolveFailed(code),
        },
    );
    if code == SOURCE_UNAVAILABLE_CODE {
        RefreshStatus::SourceUnavailable
    } else {
        RefreshStatus::ResolveFailed(code)
    }
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
        return RefreshStatus::UnknownAsset;
    };
    let Some(expected_base) = env.storage().instance().get::<_, Asset>(&DataKey::Base) else {
        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
    };

    let mut prices = AllocVec::with_capacity(config.sources.len() as usize);
    for source in config.sources.iter() {
        prices.push(source_kernel_price(env, source, &expected_base));
    }
    if prices.iter().all(Option::is_none) {
        return cache_failed_refresh(env, asset, now, SOURCE_UNAVAILABLE_CODE);
    }

    let mut breakers = match load_breakers(env, &asset) {
        Ok(breakers) => breakers,
        Err(_) => return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE),
    };
    let proxy = kernel_proxy(&config);
    let status = match proxy.resolve(&mut breakers, prices, now) {
        Ok(outcome) => match outcome.value {
            Ok(price) => match kernel_price_to_sep40(
                price,
                env.storage()
                    .instance()
                    .get(&DataKey::Decimals)
                    .unwrap_or(8_u32),
            ) {
                Ok(price) => {
                    if store_breakers(env, &asset, &breakers).is_err() {
                        return cache_failed_refresh(env, asset, now, RESOLVE_FAILED_STORAGE_CODE);
                    }
                    push_history(env, &asset, &price);
                    let cached = CachedProxyPrice {
                        updated_at: now.as_secs(),
                        status: CachedStatus::Accepted(price.clone()),
                    };
                    env.storage()
                        .persistent()
                        .set(&DataKey::Cache(asset), &cached);
                    return RefreshStatus::Accepted(price);
                }
                Err(_) => CachedStatus::ResolveFailed(CONVERSION_FAILED_CODE),
            },
            Err(reason) => CachedStatus::Blocked(blocked_reason_code(reason)),
        },
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
        require_governance(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        Ok(())
    }

    pub fn set_proxy(
        env: Env,
        asset: Asset,
        config: Option<ProxyConfig>,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        match config {
            Some(config) => {
                if config.sources.is_empty() || config.sources.len() > MAX_SOURCES_PER_PROXY {
                    return Err(ContractError::TooManySources);
                }
                if config.min_sources == 0 || config.min_sources > config.sources.len() {
                    return Err(ContractError::InvalidInput);
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
            }
            None => {
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
            }
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
        if history_len > MAX_HISTORY_RECORDS {
            return Err(ContractError::InvalidInput);
        }
        let mut breakers = load_breakers(&env, &asset)?;
        breakers.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::from_secs(sample_interval_secs),
            history_len,
        });
        store_breakers(&env, &asset, &breakers)?;
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
        breakers
            .add(breaker_id, circuit_breaker_from_config(breaker)?)
            .map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
        invalidate_cache(&env, &asset);
        Ok(breaker_id)
    }

    pub fn remove_breaker(env: Env, asset: Asset, breaker_id: u32) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        let mut breakers = load_breakers(&env, &asset)?;
        breakers.remove(breaker_id).map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
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
        breakers
            .update(breaker_id, circuit_breaker_update_from_config(update)?)
            .map_err(breaker_error)?;
        store_breakers(&env, &asset, &breakers)?;
        invalidate_cache(&env, &asset);
        Ok(())
    }

    pub fn set_manual_trip(
        env: Env,
        asset: Asset,
        is_manually_tripped: bool,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_governance(&env)?;
        require_proxy_exists(&env, &asset)?;
        let mut breakers = load_breakers(&env, &asset)?;
        breakers.set_manual_trip(
            is_manually_tripped,
            KernelAccountId::from_bytes([0_u8; 64]),
            None,
        );
        store_breakers(&env, &asset, &breakers)?;
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
            #[allow(deprecated)]
            env.events()
                .publish((symbol_short!("refresh"),), (asset.clone(), status.clone()));
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
            breaker_count: breakers.breaker_count() as u32,
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
        storage.extend_ttl(
            &DataKey::Assets,
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_EXTEND_TO,
        );
        for asset in get_assets(&env).iter() {
            storage.extend_ttl(
                &DataKey::Proxy(asset.clone()),
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            storage.extend_ttl(
                &DataKey::Breakers(asset.clone()),
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            storage.extend_ttl(
                &DataKey::Cache(asset.clone()),
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            storage.extend_ttl(
                &DataKey::History(asset.clone()),
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
        }
    }
}

#[contractimpl]
impl PriceFeedTrait for SorobanProxyOracle {
    fn base(env: Env) -> Asset {
        env.storage()
            .instance()
            .get(&DataKey::Base)
            .unwrap_or(Asset::Other(Symbol::new(&env, "USD")))
    }

    fn assets(env: Env) -> Vec<Asset> {
        get_assets(&env)
    }

    fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .unwrap_or(8)
    }

    fn resolution(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Resolution)
            .unwrap_or(1)
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
        let max_age = env
            .storage()
            .persistent()
            .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
            .and_then(|config| config.max_age_secs)
            .unwrap_or(u64::MAX);
        cached_accepted_no_older_than(&cached, max_age, env.ledger().timestamp())
    }
}

#[cfg(test)]
mod tests;
