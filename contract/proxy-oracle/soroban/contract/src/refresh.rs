//! Pull source feeds, apply refresh results, and publish events.

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
    storage::{
        cache_price, commit_history_update, load_breakers, prepare_history_update, store_breakers,
        DataKey, HistoryUpdate,
    },
    CachedProxyPrice, CachedStatus, RefreshStatus, MAX_HISTORY_RECORDS, SOURCE_UNAVAILABLE_CODE,
    STORAGE_FAILED_CODE,
};

struct RefreshComputation {
    status: RefreshStatus,
    evaluated_price: Option<NormalizedPrice>,
}

impl RefreshComputation {
    fn terminal(status: RefreshStatus) -> Self {
        Self {
            status,
            evaluated_price: None,
        }
    }
}

pub fn refresh_one(env: &Env, asset: Asset) -> RefreshStatus {
    let now = Nanoseconds::from_secs(env.ledger().timestamp());
    apply_refresh(env, &asset, now, compute_refresh(env, &asset, now))
}

fn compute_refresh(env: &Env, asset: &Asset, now: Nanoseconds) -> RefreshComputation {
    let Some(config) = env
        .storage()
        .persistent()
        .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
    else {
        return RefreshComputation::terminal(RefreshStatus::UnknownAsset);
    };
    let Some(expected_base) = env.storage().instance().get::<_, Asset>(&DataKey::Base) else {
        return RefreshComputation::terminal(RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE));
    };

    let Ok(mut breakers) = load_breakers(env, asset) else {
        return RefreshComputation::terminal(RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE));
    };
    let mut prices = AllocVec::with_capacity(config.sources.len() as usize);
    for source in config.sources.iter() {
        prices.push(source_kernel_price(env, source, &expected_base));
    }
    if prices.iter().all(Option::is_none) {
        return RefreshComputation::terminal(
            breakers
                .blocking_reason()
                .map_or(RefreshStatus::SourceUnavailable, |reason| {
                    RefreshStatus::Blocked(blocked_reason_code(reason))
                }),
        );
    }

    let outcome = match kernel_proxy_from_config(&config).resolve(&mut breakers, prices, now) {
        Ok(outcome) => outcome,
        Err(error) => {
            return RefreshComputation::terminal(RefreshStatus::ResolveFailed(resolve_error_code(
                error,
            )));
        }
    };
    let (status, history_update, evaluated_price) = match outcome.value {
        Err(reason) => (
            RefreshStatus::Blocked(blocked_reason_code(reason)),
            None,
            None,
        ),
        Ok(price) => {
            let resolved_price = kernel_price_to_normalized(price);
            match prepare_history_update(env, asset, &resolved_price, MAX_HISTORY_RECORDS) {
                HistoryUpdate::Append(update) => {
                    (RefreshStatus::Accepted(resolved_price), Some(update), None)
                }
                HistoryUpdate::Unchanged(served_price) => {
                    let evaluated_price =
                        (resolved_price != served_price).then_some(resolved_price);
                    (RefreshStatus::Accepted(served_price), None, evaluated_price)
                }
            }
        }
    };
    if store_breakers(env, asset, &breakers).is_err() {
        return RefreshComputation::terminal(RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE));
    }
    if let Some(history_update) = history_update {
        commit_history_update(env, history_update);
    }
    publish_breaker_events(env, asset, outcome.events);
    RefreshComputation {
        status,
        evaluated_price,
    }
}

fn apply_refresh(
    env: &Env,
    asset: &Asset,
    now: Nanoseconds,
    computation: RefreshComputation,
) -> RefreshStatus {
    let RefreshComputation {
        status,
        evaluated_price,
    } = computation;
    let cached_status = status_to_cached(&status);
    if let Some(cached_status) = cached_status {
        cache_price(
            env,
            asset,
            &CachedProxyPrice {
                updated_at: now.as_secs(),
                status: cached_status,
            },
        );
    }
    publish_refresh_event(env, asset, &status, evaluated_price.as_ref());
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
    if now.saturating_sub(cached.updated_at) > max_age_secs
        || now.saturating_sub(price.timestamp) > max_age_secs
    {
        return None;
    }
    Some(price.clone())
}
