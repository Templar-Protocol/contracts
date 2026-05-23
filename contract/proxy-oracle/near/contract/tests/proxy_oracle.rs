#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::unwrap_used
)]

use std::collections::HashMap;
use std::str::FromStr;

use near_sdk::{
    json_types::{Base64VecU8, I64, U64},
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, NearToken,
};
use near_workspaces::{network::Sandbox, Account, Worker};

use templar_common::{
    oracle::{
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types, Decimal, Nanoseconds,
};
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            AcceptedHistorySource, CircuitBreaker, CircuitBreakerSetConfig, PriceBlockedReason,
            StepwiseChange,
        },
        FreshnessFilter, Proxy, WeightedSource,
    },
    Price,
};
use templar_proxy_oracle_near_common::{
    cache::CachedProxyPriceStatus, governance::ProxyOracleAdminInterface, request::OracleRequest,
};
use templar_proxy_oracle_near_contract::Contract;
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
    pyth_price_id::{self, stable::CRYPTO_BTC_USD},
    worker, ContractController, MockOracleController,
};

fn norm_price(price: &pyth::Price) -> u64 {
    let p = u64::try_from(price.price.0).unwrap();
    let f = 10u64.pow(price.expo.unsigned_abs());
    if price.expo.is_negative() {
        p / f
    } else {
        p * f
    }
}

async fn update_and_list(
    proxy_oracle: &ProxyOracleController,
    actor: &Account,
    price_ids: Vec<PriceIdentifier>,
    age: u32,
) -> pyth::OracleResponse {
    proxy_oracle.update_prices(actor, price_ids.clone()).await;
    proxy_oracle
        .list_ema_prices_no_older_than(actor, price_ids, age)
        .await
}

fn proxy_price(value: i64) -> Price {
    Price {
        price: value,
        conf: 0,
        expo: 0,
        publish_time_ns: Nanoseconds::zero(),
    }
}

fn test_proxy(oracle_id: &str) -> Proxy<templar_proxy_oracle_near_common::input::Source> {
    Proxy::median_low(
        [OracleRequest::pyth(oracle_id.parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    )
}

fn cache_test_price(c: &mut Contract, price_id: PriceIdentifier, price: Price) {
    let pending = c.proxy_entry(price_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(&pending, Nanoseconds::zero(), |_, _| {
        CachedProxyPriceStatus::Accepted { price }
    })
    .unwrap();
}

fn cache_test_price_and_seed_history(c: &mut Contract, price_id: PriceIdentifier, price: Price) {
    let pending = c.proxy_entry(price_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(&pending, Nanoseconds::zero(), |_, set| {
        set.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 3,
        });
        set.try_accept_price(price, Nanoseconds::zero()).unwrap();
        CachedProxyPriceStatus::Accepted { price }
    })
    .unwrap();
}

fn stepwise_breaker() -> CircuitBreaker {
    CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::from_str("0.10").unwrap(),
    })
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn admin_upgrade_creates_one_self_receipt_with_deploy_then_migrate() {
    testing_env!(VMContextBuilder::new()
        .current_account_id("proxy.near".parse().unwrap())
        .predecessor_account_id("owner.near".parse().unwrap())
        .build());
    let mut c = Contract::new();
    let code = vec![0xde, 0xad, 0xbe, 0xef];
    let migrate_args = br#"{"from_version":"v0"}"#.to_vec();

    c.admin_upgrade(Base64VecU8(code.clone()), Base64VecU8(migrate_args.clone()))
        .detach();

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.receiver_id.as_str(), "proxy.near");
    assert!(receipt.receipt_indices.is_empty());
    assert_eq!(receipt.actions.len(), 2);

    let receipt_index = match &receipt.actions[0] {
        MockAction::DeployContract {
            receipt_index,
            code: actual_code,
        } => {
            assert_eq!(actual_code, &code);
            *receipt_index
        }
        action => panic!("expected deploy action first, got {action:?}"),
    };

    match &receipt.actions[1] {
        MockAction::FunctionCallWeight {
            receipt_index: migrate_receipt_index,
            method_name,
            args,
            attached_deposit,
            prepaid_gas,
            ..
        } => {
            assert_eq!(*migrate_receipt_index, receipt_index);
            assert_eq!(method_name, b"migrate");
            assert_eq!(args, &migrate_args);
            assert_eq!(*attached_deposit, NearToken::from_yoctonear(0));
            assert_eq!(*prepaid_gas, Contract::GAS_FOR_MIGRATE);
        }
        action => panic!("expected migrate call second, got {action:?}"),
    }
}

#[allow(clippy::unwrap_used)]
#[test]
#[should_panic(expected = "Owner only")]
pub fn admin_upgrade_requires_owner() {
    testing_env!(VMContextBuilder::new()
        .current_account_id("proxy.near".parse().unwrap())
        .predecessor_account_id("owner.near".parse().unwrap())
        .build());
    let mut c = Contract::new();

    testing_env!(VMContextBuilder::new()
        .current_account_id("proxy.near".parse().unwrap())
        .predecessor_account_id("attacker.near".parse().unwrap())
        .build());

    let _ = c.admin_upgrade(Base64VecU8(vec![0xde]), Base64VecU8(vec![]));
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn manual_trip_invalidates_cached_price() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("owner.near".parse().unwrap())
        .build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x56; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));

    let initial_epoch = c.cache_epoch(proxy_id);
    assert!(c.get_cached_proxy_price(proxy_id).is_some());

    c.admin_set_manual_trip(proxy_id, true, None);

    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
    assert_eq!(
        c.list_ema_prices_no_older_than(vec![proxy_id], 60)
            .get(&proxy_id),
        Some(&None)
    );
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn admin_configure_circuit_breakers_invalidates_cached_price() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("owner.near".parse().unwrap())
        .build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x57; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));

    let initial_epoch = c.cache_epoch(proxy_id);
    c.admin_configure_circuit_breakers(
        proxy_id,
        CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 3,
        },
    );

    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn breaker_mutations_invalidate_cached_price() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("owner.near".parse().unwrap())
        .build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x5e; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));

    cache_test_price(&mut c, proxy_id, proxy_price(100));
    let initial_epoch = c.cache_epoch(proxy_id);
    c.admin_add_circuit_breaker(proxy_id, 0, stepwise_breaker());
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);

    cache_test_price(&mut c, proxy_id, proxy_price(200));
    let initial_epoch = c.cache_epoch(proxy_id);
    c.admin_set_enforced(proxy_id, 0, false);
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);

    cache_test_price_and_seed_history(&mut c, proxy_id, proxy_price(100));
    let initial_epoch = c.cache_epoch(proxy_id);
    c.admin_rearm(
        proxy_id,
        0,
        Nanoseconds::from_secs(1),
        AcceptedHistorySource::Empty,
    );
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);

    cache_test_price(&mut c, proxy_id, proxy_price(300));
    let initial_epoch = c.cache_epoch(proxy_id);
    c.admin_remove_circuit_breaker(proxy_id, 0);
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn stale_pending_update_cannot_write_cache_or_mutate_breakers() {
    testing_env!(VMContextBuilder::new().build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x58; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-1.near")));
    let pending = c.proxy_entry(proxy_id).unwrap().prepare_price_update();

    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-2.near")));
    let breaker_set = c.get_proxy_circuit_breaker_set(proxy_id).unwrap();

    let result = c.finish_price_update_if_current(&pending, Nanoseconds::zero(), |_, set| {
        set.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 3,
        });
        CachedProxyPriceStatus::Accepted {
            price: proxy_price(100),
        }
    });

    assert_eq!(result, None);
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert_eq!(c.get_proxy_circuit_breaker_set(proxy_id), Some(breaker_set));
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn proxy_replacement_clears_cache_and_bumps_epoch() {
    testing_env!(VMContextBuilder::new().build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x59; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-1.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));

    let initial_epoch = c.cache_epoch(proxy_id);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-2.near")));

    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn proxy_removal_clears_cache_and_stale_update_cannot_write() {
    testing_env!(VMContextBuilder::new().build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x5a; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));
    let pending = c.proxy_entry(proxy_id).unwrap().prepare_price_update();

    let initial_epoch = c.cache_epoch(proxy_id);
    c.set_proxy(proxy_id, None);

    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
    assert_eq!(
        c.finish_price_update_if_current(&pending, Nanoseconds::zero(), |_, _| {
            CachedProxyPriceStatus::Accepted {
                price: proxy_price(200),
            }
        }),
        None
    );
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
}

#[allow(clippy::unwrap_used)]
#[rstest::rstest]
#[case::blocked(CachedProxyPriceStatus::Blocked {
    reason: PriceBlockedReason::ManuallyTripped,
})]
#[case::resolve_failed(CachedProxyPriceStatus::ResolveFailed {
    message: "failed".to_string(),
})]
pub fn cached_non_accepted_status_reads_as_none(#[case] status: CachedProxyPriceStatus) {
    testing_env!(VMContextBuilder::new().build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x5b; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));

    let pending = c.proxy_entry(proxy_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(&pending, Nanoseconds::zero(), |_, _| status)
        .unwrap();

    assert_eq!(
        c.list_ema_prices_no_older_than(vec![proxy_id], 60)
            .get(&proxy_id),
        Some(&None)
    );
}

#[derive(Clone, Copy)]
enum TestMethod {
    MedianLow,
    Priority,
}

#[rstest::rstest]
#[case::median_low(TestMethod::MedianLow)]
#[case::priority(TestMethod::Priority)]
#[tokio::test]
pub async fn proxy_oracle(#[future(awt)] worker: Worker<Sandbox>, #[case] method: TestMethod) {
    accounts!(
        worker,
        actor,
        redstone_adapter,
        proxy_oracle,
        pyth_oracle,
        pyth_oracle2
    );
    let pyth_oracle = MockOracleController::deploy(pyth_oracle);
    let pyth_oracle2 = MockOracleController::deploy(pyth_oracle2);
    let redstone_adapter = MockOracleController::deploy(redstone_adapter);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);
    let (pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle) =
        tokio::join!(pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle);

    let list_proxies = proxy_oracle.list_proxies(None, None).await;
    assert_eq!(list_proxies, vec![]);

    macro_rules! set {
        (pyth . $id: ident = $val: literal) => {
            set!(
                pyth.$id = Some(pyth::Price {
                    price: I64($val),
                    conf: U64(0),
                    expo: 0,
                    publish_time: PythTimestamp::from_secs(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64
                    ),
                })
            )
        };
        (pyth . $id: ident = $val: expr) => {
            pyth_oracle.set_pyth_price(&actor, pyth_price_id::stable::$id, $val)
        };
        (pyth2 . $id: ident = $val: literal) => {
            set!(
                pyth2.$id = Some(pyth::Price {
                    price: I64($val),
                    conf: U64(0),
                    expo: 0,
                    publish_time: PythTimestamp::from_secs(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64
                    ),
                })
            )
        };
        (pyth2 . $id: ident = $val: expr) => {
            pyth_oracle2.set_pyth_price(&actor, pyth_price_id::stable::$id, $val)
        };
        (redstone . $id: ident = $val: literal) => {
            set!(
                redstone.$id = Some(FeedData {
                    price: primitive_types::U256::from($val * 100_000_000_u128).into(),
                    package_timestamp: templar_common::Nanoseconds::from_ms(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                    write_timestamp: templar_common::Nanoseconds::from_ms(
                        std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64
                    ),
                })
            )
        };
        (redstone . $id: ident = $val: expr) => {
            redstone_adapter.set_redstone_price(&actor, stringify!($id), $val)
        };
    }

    let default_filter = FreshnessFilter::new(
        Some(Nanoseconds::from_ms(60 * 1000)),
        Some(Nanoseconds::from_ms(10 * 1000)),
    );

    let btc_proxy_def = match method {
        TestMethod::MedianLow => Proxy::new(
            Aggregator::MedianLow(MedianLow::new([
                WeightedSource::new(
                    OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD),
                    1,
                ),
                WeightedSource::new(
                    OracleRequest::redstone(redstone_adapter.id().clone(), "BTC"),
                    1,
                ),
                WeightedSource::new(
                    OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD),
                    1,
                ),
            ])),
            default_filter.clone(),
        ),
        TestMethod::Priority => Proxy::priority(
            [
                OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD).into(),
                OracleRequest::redstone(redstone_adapter.id().clone(), "BTC").into(),
                OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into(),
            ],
            default_filter.clone(),
        ),
    };
    let btc_proxy_id = PriceIdentifier([0x01_u8; 32]);

    // Single-source proxies: method doesn't affect the result.
    let just_pyth_btc = Proxy::median_low(
        [OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()],
        default_filter.clone(),
    );
    let just_pyth_btc_id = PriceIdentifier([0x02_u8; 32]);
    let just_redstone_eth = Proxy::median_low(
        [OracleRequest::redstone(redstone_adapter.id().clone(), "ETH").into()],
        default_filter.clone(),
    );
    let just_redstone_eth_id = PriceIdentifier([0x03_u8; 32]);

    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            btc_proxy_id,
            Some(btc_proxy_def.clone()),
        )
        .await;
    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            just_pyth_btc_id,
            Some(just_pyth_btc.clone()),
        )
        .await;
    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            just_redstone_eth_id,
            Some(just_redstone_eth.clone()),
        )
        .await;

    assert_eq!(
        proxy_oracle.list_proxies(None, None).await,
        vec![btc_proxy_id, just_pyth_btc_id, just_redstone_eth_id],
    );
    assert_eq!(
        proxy_oracle.get_proxy(btc_proxy_id).await.unwrap(),
        btc_proxy_def,
    );

    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![btc_proxy_id, CRYPTO_BTC_USD], 60_u32)
        .await;
    assert_eq!(result, HashMap::from_iter([(btc_proxy_id, None)]));

    // Step 1: Only redstone has a price. Single source → same for both methods.
    set!(redstone.BTC = 100_000).await;
    let result = update_and_list(
        &proxy_oracle,
        &actor,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60_u32,
    )
    .await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(100_000),
    );

    // Step 2: redstone=100k, pyth1=90k.
    //   MedianLow: median of [90k, 100k] → 90k
    //   Priority: redstone (weight 5) > pyth1 (weight 1) → 100k
    set!(pyth.CRYPTO_BTC_USD = 90_000).await;
    set!(redstone.ETH = 1_800).await;
    let result = update_and_list(
        &proxy_oracle,
        &actor,
        vec![
            btc_proxy_id,
            CRYPTO_BTC_USD,
            btc_proxy_id,
            CRYPTO_BTC_USD,
            CRYPTO_BTC_USD,
            just_pyth_btc_id,
            just_redstone_eth_id,
        ],
        60_u32,
    )
    .await;
    assert_eq!(result.len(), 3);
    let expected_btc_2source = match method {
        TestMethod::MedianLow => 90_000,
        TestMethod::Priority => 100_000,
    };
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(expected_btc_2source),
    );
    assert_eq!(
        result
            .get(&just_pyth_btc_id)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(90_000),
    );
    assert_eq!(
        result
            .get(&just_redstone_eth_id)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(1_800),
    );

    // Step 3: All three sources: pyth1=90k, redstone=100k, pyth2=80k.
    //   MedianLow: median of [80k, 90k, 100k] → 90k
    //   Priority: pyth2 (weight 10) wins → 80k
    set!(pyth2.CRYPTO_BTC_USD = 80_000).await;
    let result = update_and_list(
        &proxy_oracle,
        &actor,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60_u32,
    )
    .await;
    assert_eq!(result.len(), 1);
    let expected_btc_3source = match method {
        TestMethod::MedianLow => 90_000,
        TestMethod::Priority => 80_000,
    };
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(expected_btc_3source),
    );

    // Step 4: Clear pyth1 and redstone, only pyth2=80k remains. Single source → same for both.
    set!(pyth.CRYPTO_BTC_USD = None).await;
    set!(redstone.BTC = None).await;
    let result = update_and_list(
        &proxy_oracle,
        &actor,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60_u32,
    )
    .await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(80_000),
    );
}

#[rstest::rstest]
#[case::median_low(TestMethod::MedianLow, 100_000)]
#[case::priority(TestMethod::Priority, 100_000)]
#[tokio::test]
async fn proxy_oracle_enforces_freshness_filter(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] method: TestMethod,
    #[case] expected_price: u64,
) {
    accounts!(
        worker,
        actor,
        redstone_adapter,
        proxy_oracle,
        pyth_oracle,
        pyth_oracle2
    );
    let pyth_oracle = MockOracleController::deploy(pyth_oracle);
    let pyth_oracle2 = MockOracleController::deploy(pyth_oracle2);
    let redstone_adapter = MockOracleController::deploy(redstone_adapter);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle);
    let (pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle) =
        tokio::join!(pyth_oracle, pyth_oracle2, redstone_adapter, proxy_oracle);

    let default_filter = FreshnessFilter::new(
        Some(Nanoseconds::from_secs(10)),
        Some(Nanoseconds::from_secs(10)),
    );

    let btc_proxy_def = match method {
        TestMethod::MedianLow => Proxy::new(
            Aggregator::MedianLow(MedianLow::new([
                WeightedSource::new(
                    OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD),
                    1,
                ),
                WeightedSource::new(
                    OracleRequest::redstone(redstone_adapter.id().clone(), "BTC"),
                    1,
                ),
                WeightedSource::new(
                    OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD),
                    1,
                ),
            ])),
            default_filter,
        ),
        TestMethod::Priority => Proxy::priority(
            [
                OracleRequest::pyth(pyth_oracle2.id().clone(), CRYPTO_BTC_USD).into(),
                OracleRequest::redstone(redstone_adapter.id().clone(), "BTC").into(),
                OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into(),
            ],
            default_filter,
        ),
    };
    let btc_proxy_id = PriceIdentifier([0x09_u8; 32]);
    proxy_oracle
        .admin_set_proxy(
            proxy_oracle.account(),
            btc_proxy_id,
            Some(btc_proxy_def.clone()),
        )
        .await;

    let now = Nanoseconds::from_ms(std::time::UNIX_EPOCH.elapsed().unwrap().as_millis() as u64);
    let stale_time = now.saturating_sub(Nanoseconds::from_secs(30));
    let future_time = now.saturating_add(Nanoseconds::from_secs(20));

    pyth_oracle
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(expected_price as i64),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::try_from_time(now).unwrap(),
            }),
        )
        .await;
    pyth_oracle2
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(80_000),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::try_from_time(future_time).unwrap(),
            }),
        )
        .await;
    redstone_adapter
        .set_redstone_price(
            &actor,
            "BTC",
            Some(FeedData {
                price: primitive_types::U256::from(90_000_u128 * 100_000_000_u128).into(),
                package_timestamp: stale_time,
                write_timestamp: stale_time,
            }),
        )
        .await;

    let result = update_and_list(&proxy_oracle, &actor, vec![btc_proxy_id], 60_u32).await;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(expected_price)
    );
}
