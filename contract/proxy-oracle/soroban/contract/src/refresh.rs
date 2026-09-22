//! Pull source feeds, apply refresh results, and publish events.

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::Env;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{proxy::FreshnessFilter, Price};
use templar_proxy_oracle_soroban_common::{
    Asset, ContractError, NormalizedPrice, PriceFeedClient, ProxyConfig, SourceConfig,
};

use crate::{
    codes::{blocked_reason_code, resolve_error_code},
    conversion::{kernel_price_to_normalized, kernel_proxy_from_config, source_price_to_kernel},
    events::{publish_breaker_events, publish_refresh_event},
    storage::{
        cache_price, commit_history_update, load_breakers, prepare_history_update, store_breakers,
        DataKey, HistoryUpdate,
    },
    AcceptedPrice, CachedProxyPrice, CachedStatus, RefreshStatus, MAX_HISTORY_RECORDS,
    NON_ADVANCING_WITHOUT_PROOF_CODE, SOURCE_UNAVAILABLE_CODE, STORAGE_FAILED_CODE,
};

struct SourceObservation {
    price: Price,
    valid_until: u64,
}

struct RefreshComputation {
    status: RefreshStatus,
    evaluated_price: Option<NormalizedPrice>,
    cache_write: Option<CachedStatus>,
}

impl RefreshComputation {
    fn terminal(status: RefreshStatus) -> Self {
        Self {
            cache_write: terminal_cache_status(&status),
            status,
            evaluated_price: None,
        }
    }
}

pub fn refresh_one(env: &Env, asset: Asset) -> RefreshStatus {
    let now = Nanoseconds::checked_from_secs(env.ledger().timestamp())
        .unwrap_or_else(|| env.panic_with_error(ContractError::ConversionOverflow));
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
    let mut valid_until = now
        .as_secs()
        .checked_add(config.max_cache_age_secs)
        .unwrap_or(u64::MAX);
    for source in config.sources.iter() {
        let observation = source_observation(env, source, &expected_base, now);
        prices.push(observation.map(|observation| {
            valid_until = valid_until.min(observation.valid_until);
            observation.price
        }));
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
    let (status, history_update, evaluated_price, cache_write) = match outcome.value {
        Err(reason) => {
            let status = RefreshStatus::Blocked(blocked_reason_code(reason));
            let cache_write = terminal_cache_status(&status);
            (status, None, None, cache_write)
        }
        Ok(price) => {
            let candidate = kernel_price_to_normalized(price);
            match prepare_history_update(env, asset, &candidate, MAX_HISTORY_RECORDS) {
                HistoryUpdate::Append(update) => {
                    let status = RefreshStatus::Accepted(candidate.clone());
                    let cache_write = Some(CachedStatus::Accepted(AcceptedPrice {
                        price: candidate,
                        valid_until,
                    }));
                    (status, Some(update), None, cache_write)
                }
                HistoryUpdate::Unchanged(served) if candidate == served => {
                    let status = RefreshStatus::Accepted(served.clone());
                    let cache_write = Some(CachedStatus::Accepted(AcceptedPrice {
                        price: served,
                        valid_until,
                    }));
                    (status, None, None, cache_write)
                }
                HistoryUpdate::Unchanged(served) => {
                    let proof_is_live = env
                        .storage()
                        .persistent()
                        .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))
                        .and_then(|cached| cached_accepted_if_valid(&cached, now.as_secs()))
                        .is_some_and(|cached| cached == served);
                    if proof_is_live {
                        (RefreshStatus::Accepted(served), None, Some(candidate), None)
                    } else {
                        let status = RefreshStatus::ResolveFailed(NON_ADVANCING_WITHOUT_PROOF_CODE);
                        let cache_write = terminal_cache_status(&status);
                        (status, None, Some(candidate), cache_write)
                    }
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
        cache_write,
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
        cache_write,
    } = computation;
    if let Some(status) = cache_write {
        cache_price(
            env,
            asset,
            &CachedProxyPrice {
                updated_at: now.as_secs(),
                status,
            },
        );
    }
    publish_refresh_event(env, asset, &status, evaluated_price.as_ref());
    status
}

fn terminal_cache_status(status: &RefreshStatus) -> Option<CachedStatus> {
    match status {
        RefreshStatus::UnknownAsset | RefreshStatus::Accepted(_) => None,
        RefreshStatus::Blocked(code) => Some(CachedStatus::Blocked(*code)),
        RefreshStatus::ResolveFailed(code) => Some(CachedStatus::ResolveFailed(*code)),
        RefreshStatus::SourceUnavailable => {
            Some(CachedStatus::ResolveFailed(SOURCE_UNAVAILABLE_CODE))
        }
    }
}

fn source_observation(
    env: &Env,
    source: SourceConfig,
    expected_base: &Asset,
    now: Nanoseconds,
) -> Option<SourceObservation> {
    let client = PriceFeedClient::new(env, &source.oracle);
    let base = client.try_base().ok().and_then(Result::ok)?;
    if &base != expected_base {
        return None;
    }
    let decimals = client.try_decimals().ok().and_then(Result::ok)?;
    let source_price = client
        .try_lastprice(&source.asset)
        .ok()
        .and_then(Result::ok)
        .flatten()?;
    let valid_until = source_price
        .timestamp
        .checked_add(source.max_age_secs)
        .unwrap_or(u64::MAX);
    let price = source_price_to_kernel(source_price, decimals).ok()?;
    FreshnessFilter::new(
        Some(Nanoseconds::from_secs(source.max_age_secs)),
        Some(Nanoseconds::from_secs(source.max_clock_drift_secs)),
    )
    .accepts(&price, now)
    .then_some(SourceObservation { price, valid_until })
}

pub fn cached_accepted_if_valid(cached: &CachedProxyPrice, now: u64) -> Option<NormalizedPrice> {
    let CachedStatus::Accepted(accepted) = &cached.status else {
        return None;
    };
    (now <= accepted.valid_until).then_some(accepted.price.clone())
}
