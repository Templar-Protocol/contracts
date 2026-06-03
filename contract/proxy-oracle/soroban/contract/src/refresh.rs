//! Pull source feeds, run the kernel resolve, cache the outcome, and emit
//! events. `compute_refresh` runs the resolve, persists the updated breaker
//! state, and publishes breaker events; `apply_refresh` performs the
//! cache write, accepted-price history push, and the top-level refresh
//! event. Splitting the two leaves the cache update + refresh-event emission
//! sequenced after every successful resolve, but the breaker-side effects
//! must run inline with the resolve to stay consistent with the kernel
//! state that was just computed.

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::Env;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::Price;
use templar_proxy_oracle_soroban_common::{
    Asset, NormalizedPrice, PriceFeedClient, ProxyConfig, SourceConfig,
};

use crate::{
    codes::{blocked_reason_code, resolve_error_code},
    conversion::{kernel_price_to_normalized, kernel_proxy_from_config, source_price_to_kernel},
    events::{publish_breaker_events, publish_refresh_event},
    storage::{cache_price, load_breakers, push_history, store_breakers, DataKey},
    CachedProxyPrice, CachedStatus, RefreshStatus, MAX_HISTORY_RECORDS, SOURCE_UNAVAILABLE_CODE,
    STORAGE_FAILED_CODE,
};

pub fn refresh_one(env: &Env, asset: Asset) -> RefreshStatus {
    let now = Nanoseconds::from_secs(env.ledger().timestamp());
    let status = compute_refresh(env, &asset, now);
    apply_refresh(env, &asset, now, status)
}

fn compute_refresh(env: &Env, asset: &Asset, now: Nanoseconds) -> RefreshStatus {
    let Some(config) = env
        .storage()
        .persistent()
        .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
    else {
        return RefreshStatus::UnknownAsset;
    };
    let Some(expected_base) = env.storage().instance().get::<_, Asset>(&DataKey::Base) else {
        return RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE);
    };

    let mut prices = AllocVec::with_capacity(config.sources.len() as usize);
    for source in config.sources.iter() {
        prices.push(source_kernel_price(env, source, &expected_base));
    }
    if prices.iter().all(Option::is_none) {
        return RefreshStatus::SourceUnavailable;
    }

    let Ok(mut breakers) = load_breakers(env, asset) else {
        return RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE);
    };
    let resolve_result = kernel_proxy_from_config(&config).resolve(&mut breakers, prices, now);
    if store_breakers(env, asset, &breakers).is_err() {
        return RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE);
    }

    let outcome = match resolve_result {
        Ok(outcome) => outcome,
        Err(error) => return RefreshStatus::ResolveFailed(resolve_error_code(error)),
    };
    publish_breaker_events(env, asset, outcome.events);

    match outcome.value {
        Err(reason) => RefreshStatus::Blocked(blocked_reason_code(reason)),
        Ok(price) => RefreshStatus::Accepted(kernel_price_to_normalized(price)),
    }
}

fn apply_refresh(
    env: &Env,
    asset: &Asset,
    now: Nanoseconds,
    status: RefreshStatus,
) -> RefreshStatus {
    if let Some(cached_status) = status_to_cached(&status) {
        if let RefreshStatus::Accepted(price) = &status {
            push_history(env, asset, price, MAX_HISTORY_RECORDS);
        }
        cache_price(
            env,
            asset,
            &CachedProxyPrice {
                updated_at: now.as_secs(),
                status: cached_status,
            },
        );
    }
    publish_refresh_event(env, asset, &status);
    status
}

fn status_to_cached(status: &RefreshStatus) -> Option<CachedStatus> {
    match status {
        RefreshStatus::UnknownAsset => None,
        RefreshStatus::Accepted(price) => Some(CachedStatus::Accepted(price.clone())),
        RefreshStatus::Blocked(code) => Some(CachedStatus::Blocked(*code)),
        RefreshStatus::ResolveFailed(code) => Some(CachedStatus::ResolveFailed(*code)),
        RefreshStatus::SourceUnavailable => {
            Some(CachedStatus::ResolveFailed(SOURCE_UNAVAILABLE_CODE))
        }
    }
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

pub fn cached_accepted_no_older_than(
    cached: &CachedProxyPrice,
    max_age_secs: u64,
    now: u64,
) -> Option<NormalizedPrice> {
    let CachedStatus::Accepted(price) = &cached.status else {
        return None;
    };
    // Bound staleness by the cache update time, not the upstream source's
    // claimed timestamp. A source returning a future-dated reading would
    // otherwise bypass the freshness gate forever (`now < price.timestamp`
    // makes `saturating_sub` zero).
    if now.saturating_sub(cached.updated_at) > max_age_secs {
        return None;
    }
    Some(price.clone())
}
