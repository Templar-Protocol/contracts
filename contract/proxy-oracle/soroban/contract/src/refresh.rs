//! Pull source feeds, apply refresh results, and publish events.

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{Env, Vec};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::method::Error as AggregationError, circuit_breaker::PriceBlockedReason,
        ResolveError,
    },
    Price,
};
use templar_proxy_oracle_soroban_common::{
    Asset, ContractError, NormalizedPrice, PriceFeedClient, ProxyConfig, SourceConfig,
};

use crate::{
    codes::{blocked_reason_code, resolve_error_code},
    conversion::{kernel_price_to_normalized, source_price_to_kernel},
    events::{
        publish_breaker_events, AggregationFailureDetail, CacheBlocked, CacheBlockedDetail,
        RefreshEvaluated, RefreshFailure, RefreshSuccess,
    },
    storage::{
        cache_price, commit_history_update, load_breakers, prepare_history_update, store_breakers,
        DataKey, HistoryUpdate,
    },
    CachedProxyPrice, CachedStatus, RefreshStatus, MAX_HISTORY_RECORDS, SOURCE_UNAVAILABLE_CODE,
    STORAGE_FAILED_CODE, UNKNOWN_ASSET_CODE,
};

enum RefreshComputation {
    Accepted {
        price: NormalizedPrice,
        evaluated_price: Option<NormalizedPrice>,
    },
    Blocked(PriceBlockedReason),
    ResolveFailed(ResolveError),
    SourceUnavailable,
    UnknownAsset,
    StorageFailed,
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
        return RefreshComputation::UnknownAsset;
    };
    let Some(expected_base) = env.storage().instance().get::<_, Asset>(&DataKey::Base) else {
        return RefreshComputation::StorageFailed;
    };

    let Ok(mut breakers) = load_breakers(env, asset) else {
        return RefreshComputation::StorageFailed;
    };
    let proxy = config.to_kernel();
    let mut prices = AllocVec::with_capacity(proxy.sources().len());
    for source in proxy.sources() {
        prices.push(source_kernel_price(env, source, &expected_base));
    }
    if prices.iter().all(Option::is_none) {
        return breakers.blocking_reason().map_or(
            RefreshComputation::SourceUnavailable,
            RefreshComputation::Blocked,
        );
    }

    let outcome = match proxy.resolve(&mut breakers, prices, now) {
        Ok(outcome) => outcome,
        Err(error) => return RefreshComputation::ResolveFailed(error),
    };
    let (computation, history_update) = match outcome.value {
        Err(reason) => (RefreshComputation::Blocked(reason), None),
        Ok(price) => {
            let resolved_price = kernel_price_to_normalized(price);
            match prepare_history_update(env, asset, &resolved_price, MAX_HISTORY_RECORDS) {
                HistoryUpdate::Append(update) => (
                    RefreshComputation::Accepted {
                        price: resolved_price,
                        evaluated_price: None,
                    },
                    Some(update),
                ),
                HistoryUpdate::Unchanged(served_price) => {
                    let evaluated_price =
                        (resolved_price != served_price).then_some(resolved_price);
                    (
                        RefreshComputation::Accepted {
                            price: served_price,
                            evaluated_price,
                        },
                        None,
                    )
                }
            }
        }
    };
    if store_breakers(env, asset, &breakers).is_err() {
        return RefreshComputation::StorageFailed;
    }
    if let Some(history_update) = history_update {
        commit_history_update(env, history_update);
    }
    publish_breaker_events(env, asset, outcome.events);
    computation
}

fn publish_refresh_failure(env: &Env, asset: &Asset, code: u32) {
    RefreshFailure {
        asset: asset.clone(),
        code,
    }
    .publish(env);
}

fn apply_refresh(
    env: &Env,
    asset: &Asset,
    now: Nanoseconds,
    computation: RefreshComputation,
) -> RefreshStatus {
    let (status, cached_status) = match computation {
        RefreshComputation::Accepted {
            price,
            evaluated_price,
        } => {
            if let Some(evaluated) = evaluated_price {
                RefreshEvaluated {
                    asset: asset.clone(),
                    mantissa: evaluated.mantissa,
                    expo: evaluated.expo,
                    timestamp: evaluated.timestamp,
                }
                .publish(env);
            }
            RefreshSuccess {
                asset: asset.clone(),
                mantissa: price.mantissa,
                expo: price.expo,
                timestamp: price.timestamp,
            }
            .publish(env);
            let cached = CachedStatus::Accepted(price.clone());
            (RefreshStatus::Accepted(price), Some(cached))
        }
        RefreshComputation::Blocked(reason) => {
            let code = blocked_reason_code(&reason);
            CacheBlocked {
                asset: asset.clone(),
                reason_code: code,
            }
            .publish(env);
            let ids = match reason {
                PriceBlockedReason::ManuallyTripped => Vec::new(env),
                PriceBlockedReason::BreakerTripped {
                    blocking_breaker_ids,
                } => Vec::from_slice(env, &blocking_breaker_ids),
            };
            CacheBlockedDetail {
                asset: asset.clone(),
                blocking_breaker_ids: ids,
            }
            .publish(env);
            (
                RefreshStatus::Blocked(code),
                Some(CachedStatus::Blocked(code)),
            )
        }
        RefreshComputation::ResolveFailed(error) => {
            let code = resolve_error_code(&error);
            publish_refresh_failure(env, asset, code);
            if let ResolveError::Aggregation(
                AggregationError::LengthMismatch { expected, actual }
                | AggregationError::TooFewValidSources { expected, actual },
            ) = error
            {
                // Both aligned inputs and configured sources are bounded to 16.
                AggregationFailureDetail {
                    asset: asset.clone(),
                    code,
                    expected: u32::try_from(expected)
                        .unwrap_or_else(|_| env.panic_with_error(ContractError::StorageError)),
                    actual: u32::try_from(actual)
                        .unwrap_or_else(|_| env.panic_with_error(ContractError::StorageError)),
                }
                .publish(env);
            }
            (
                RefreshStatus::ResolveFailed(code),
                Some(CachedStatus::ResolveFailed(code)),
            )
        }
        RefreshComputation::SourceUnavailable => {
            publish_refresh_failure(env, asset, SOURCE_UNAVAILABLE_CODE);
            (
                RefreshStatus::SourceUnavailable,
                Some(CachedStatus::ResolveFailed(SOURCE_UNAVAILABLE_CODE)),
            )
        }
        RefreshComputation::UnknownAsset => {
            publish_refresh_failure(env, asset, UNKNOWN_ASSET_CODE);
            (RefreshStatus::UnknownAsset, None)
        }
        RefreshComputation::StorageFailed => {
            publish_refresh_failure(env, asset, STORAGE_FAILED_CODE);
            (
                RefreshStatus::ResolveFailed(STORAGE_FAILED_CODE),
                Some(CachedStatus::ResolveFailed(STORAGE_FAILED_CODE)),
            )
        }
    };
    if let Some(status) = cached_status {
        cache_price(
            env,
            asset,
            &CachedProxyPrice {
                updated_at: now.as_secs(),
                status,
            },
        );
    }
    status
}

fn source_kernel_price(env: &Env, source: &SourceConfig, expected_base: &Asset) -> Option<Price> {
    let client = PriceFeedClient::new(env, &source.oracle);
    let base = client.try_base().ok()?.ok()?;
    if &base != expected_base {
        return None;
    }
    let decimals = client.try_decimals().ok()?.ok()?;
    if decimals != source.expected_decimals {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use alloc::vec;
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        Address, Event, Symbol,
    };
    use templar_proxy_oracle_kernel::proxy::{
        circuit_breaker::{CircuitBreakerError, CircuitBreakerSet},
        Proxy,
    };

    use super::*;
    use crate::{SorobanProxyOracle, AGGREGATION_LENGTH_MISMATCH_CODE};

    #[test]
    fn length_mismatch_projects_checked_counts_and_generic_failure_has_no_detail() {
        let env = Env::default();
        env.mock_all_auths();
        let asset = Asset::Other(Symbol::new(&env, "BTC"));
        let base = Asset::Other(Symbol::new(&env, "USD"));
        let contract = env.register(SorobanProxyOracle, (&Address::generate(&env), &base));
        // The contract IO creates aligned vectors, so exercise the genuine
        // kernel length error directly at the runtime projection boundary.
        let proxy = Proxy::priority(
            [0_u32, 1, 2],
            templar_proxy_oracle_kernel::proxy::FreshnessFilter::empty(),
        );
        let mut breakers: CircuitBreakerSet = CircuitBreakerSet::empty();
        let error = proxy
            .resolve(&mut breakers, vec![None], Nanoseconds::zero())
            .unwrap_err();
        assert!(matches!(
            &error,
            ResolveError::Aggregation(AggregationError::LengthMismatch {
                expected: 3,
                actual: 1,
            })
        ));
        let result = env.as_contract(&contract, || {
            apply_refresh(
                &env,
                &asset,
                Nanoseconds::zero(),
                RefreshComputation::ResolveFailed(error),
            )
        });
        assert!(matches!(result, RefreshStatus::ResolveFailed(101)));
        let events = env
            .events()
            .all()
            .filter_by_contract(&contract)
            .events()
            .to_vec();
        assert_eq!(
            events,
            vec![
                RefreshFailure {
                    asset: asset.clone(),
                    code: AGGREGATION_LENGTH_MISMATCH_CODE,
                }
                .to_xdr(&env, &contract),
                AggregationFailureDetail {
                    asset: asset.clone(),
                    code: AGGREGATION_LENGTH_MISMATCH_CODE,
                    expected: 3,
                    actual: 1,
                }
                .to_xdr(&env, &contract),
            ]
        );
        for computation in [
            RefreshComputation::StorageFailed,
            RefreshComputation::ResolveFailed(ResolveError::CircuitBreaker(
                CircuitBreakerError::InvalidPrice,
            )),
        ] {
            let result = env.as_contract(&contract, || {
                apply_refresh(&env, &asset, Nanoseconds::zero(), computation)
            });
            assert!(matches!(result, RefreshStatus::ResolveFailed(3)));
            assert_eq!(
                env.events()
                    .all()
                    .filter_by_contract(&contract)
                    .events()
                    .to_vec(),
                vec![RefreshFailure {
                    asset: asset.clone(),
                    code: STORAGE_FAILED_CODE,
                }
                .to_xdr(&env, &contract)]
            );
        }
    }
}
