//! Storage layout for the runtime contract.
//!
//! `instance` storage holds singleton config (governance, base, decimals,
//! resolution); `persistent` storage is keyed per-asset (proxy config,
//! breaker set, price cache, history).

use soroban_sdk::{contracttype, Address, Bytes, Env, Vec};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::CircuitBreakerSet;
use templar_proxy_oracle_soroban_common::{
    Asset, ContractError, DEFAULT_TTL_EXTEND_TO, DEFAULT_TTL_THRESHOLD,
};

use crate::{CachedProxyPrice, PriceData};

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
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

pub fn require_governance(env: &Env) -> Result<Address, ContractError> {
    let governance: Address = env
        .storage()
        .instance()
        .get(&DataKey::Governance)
        .ok_or(ContractError::MissingConfig)?;
    governance.require_auth();
    Ok(governance)
}

pub fn require_proxy_exists(env: &Env, asset: &Asset) -> Result<(), ContractError> {
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

pub fn invalidate_cache(env: &Env, asset: &Asset) {
    env.storage()
        .persistent()
        .remove(&DataKey::Cache(asset.clone()));
}

pub fn get_assets(env: &Env) -> Vec<Asset> {
    env.storage()
        .persistent()
        .get(&DataKey::Assets)
        .unwrap_or_else(|| Vec::new(env))
}

pub fn add_asset(env: &Env, asset: &Asset) {
    let mut assets = get_assets(env);
    if !assets.iter().any(|entry| &entry == asset) {
        assets.push_back(asset.clone());
        env.storage().persistent().set(&DataKey::Assets, &assets);
    }
}

pub fn remove_asset(env: &Env, asset: &Asset) {
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

pub fn load_breakers(env: &Env, asset: &Asset) -> Result<CircuitBreakerSet, ContractError> {
    let Some(bytes) = env
        .storage()
        .persistent()
        .get::<_, Bytes>(&DataKey::Breakers(asset.clone()))
    else {
        return Ok(CircuitBreakerSet::empty());
    };
    postcard::from_bytes(&bytes.to_alloc_vec()).map_err(|_| ContractError::StorageError)
}

pub fn store_breakers(
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

pub fn cache_price(env: &Env, asset: &Asset, cached: &CachedProxyPrice) {
    env.storage()
        .persistent()
        .set(&DataKey::Cache(asset.clone()), cached);
}

pub fn push_history(env: &Env, asset: &Asset, price: &PriceData, max_records: u32) {
    let key = DataKey::History(asset.clone());
    let mut history = env
        .storage()
        .persistent()
        .get::<_, Vec<PriceData>>(&key)
        .unwrap_or_else(|| Vec::new(env));
    if let Some(index) = history
        .iter()
        .position(|entry| entry.timestamp == price.timestamp)
        .and_then(|i| u32::try_from(i).ok())
    {
        history.remove(index);
    }
    history.push_back(price.clone());
    while history.len() > max_records {
        history.pop_front();
    }
    env.storage().persistent().set(&key, &history);
}

pub fn extend_persistent_ttl(env: &Env, key: &DataKey) {
    let storage = env.storage().persistent();
    if storage.has(key) {
        storage.extend_ttl(key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
}
