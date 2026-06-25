#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::unwrap_used
)]

mod common;

use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Result;
use near_api::{types::AccountId, NetworkConfig};
use near_sdk::{
    json_types::Base64VecU8,
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, NearToken,
};
use serde_json::json;
use templar_common::{
    oracle::pyth::{self, OracleResponse, PriceIdentifier},
    Decimal, Nanoseconds,
};
use templar_gateway_testing::SandboxHarness;
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
    cache::CachedProxyPriceStatus, governance::ProxyOracleAdminInterface, input::Source,
    request::OracleRequest,
};
use templar_proxy_oracle_near_contract::Contract;
use test_utils::pyth_price_id::stable::CRYPTO_BTC_USD;

// ---------------------------------------------------------------------------
// Pure-unit tests (near-sdk `testing_env!`, no sandbox).
// ---------------------------------------------------------------------------

fn norm_price(price: &pyth::Price) -> u64 {
    let p = u64::try_from(price.price.0).unwrap();
    let f = 10u64.pow(price.expo.unsigned_abs());
    if price.expo.is_negative() {
        p / f
    } else {
        p * f
    }
}

fn proxy_price(value: i64) -> Price {
    Price {
        price: value,
        conf: 0,
        expo: 0,
        publish_time_ns: Nanoseconds::zero(),
    }
}

fn test_proxy(oracle_id: &str) -> Proxy<Source> {
    Proxy::median_low(
        [OracleRequest::pyth(oracle_id.parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    )
}

fn cache_test_price(c: &mut Contract, price_id: PriceIdentifier, price: Price) {
    let pending = c.proxy_entry(price_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| {
        CachedProxyPriceStatus::Accepted { price }
    })
    .unwrap();
}

fn cache_test_price_and_seed_history(c: &mut Contract, price_id: PriceIdentifier, price: Price) {
    let pending = c.proxy_entry(price_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, set| {
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

#[test]
pub fn stale_pending_update_cannot_write_cache_or_mutate_breakers() {
    testing_env!(VMContextBuilder::new().build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x58; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-1.near")));
    let pending = c.proxy_entry(proxy_id).unwrap().prepare_price_update();

    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle-2.near")));
    let breaker_set = c.get_proxy_circuit_breaker_set(proxy_id).unwrap();

    let result = c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, set| {
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
        c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| {
            CachedProxyPriceStatus::Accepted {
                price: proxy_price(200),
            }
        }),
        None
    );
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
}

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
    c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| status)
        .unwrap();

    assert_eq!(
        c.list_ema_prices_no_older_than(vec![proxy_id], 60)
            .get(&proxy_id),
        Some(&None)
    );
}

// ---------------------------------------------------------------------------
// Sandbox tests (gateway `SandboxHarness`).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum TestMethod {
    MedianLow,
    Priority,
}

/// Push fresh prices into the proxy oracle's cache, then read them back.
async fn update_and_list(
    network: &NetworkConfig,
    proxy_id: &AccountId,
    price_ids: Vec<PriceIdentifier>,
    age: u64,
) -> Result<OracleResponse> {
    common::call(
        network,
        proxy_id,
        proxy_id,
        "update_prices",
        json!({ "price_ids": price_ids }),
        300,
        0,
    )
    .await?;
    common::view(
        network,
        proxy_id,
        "list_ema_prices_no_older_than",
        json!({ "price_ids": price_ids, "age": age }),
    )
    .await
}

#[rstest::rstest]
#[case::median_low(TestMethod::MedianLow)]
#[case::priority(TestMethod::Priority)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
#[allow(clippy::too_many_lines)]
async fn proxy_oracle(#[case] method: TestMethod) -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = harness.network.clone();

    let pyth_oracle = harness
        .deploy_mock_oracle("pyth-oracle.near".parse()?)
        .await?;
    let pyth_oracle2 = harness
        .deploy_mock_oracle("pyth-oracle2.near".parse()?)
        .await?;
    let redstone_adapter = harness
        .deploy_mock_oracle("redstone-adapter.near".parse()?)
        .await?;
    let proxy_oracle = harness.deploy_proxy_oracle().await?;

    let list_proxies: Vec<PriceIdentifier> = common::view(
        &network,
        &proxy_oracle,
        "list_proxies",
        json!({ "offset": null, "count": null }),
    )
    .await?;
    assert_eq!(list_proxies, vec![]);

    let default_filter = FreshnessFilter::new(
        Some(Nanoseconds::from_ms(60 * 1000)),
        Some(Nanoseconds::from_ms(10 * 1000)),
    );

    let btc_proxy_def = match method {
        TestMethod::MedianLow => Proxy::new(
            Aggregator::MedianLow(MedianLow::new([
                WeightedSource::new(OracleRequest::pyth(pyth_oracle.clone(), CRYPTO_BTC_USD), 1),
                WeightedSource::new(OracleRequest::redstone(redstone_adapter.clone(), "BTC"), 1),
                WeightedSource::new(OracleRequest::pyth(pyth_oracle2.clone(), CRYPTO_BTC_USD), 1),
            ])),
            default_filter.clone(),
        ),
        TestMethod::Priority => Proxy::priority(
            [
                OracleRequest::pyth(pyth_oracle2.clone(), CRYPTO_BTC_USD).into(),
                OracleRequest::redstone(redstone_adapter.clone(), "BTC").into(),
                OracleRequest::pyth(pyth_oracle.clone(), CRYPTO_BTC_USD).into(),
            ],
            default_filter.clone(),
        ),
    };
    let btc_proxy_id = PriceIdentifier([0x01_u8; 32]);

    // Single-source proxies: method doesn't affect the result.
    let just_pyth_btc = Proxy::median_low(
        [OracleRequest::pyth(pyth_oracle.clone(), CRYPTO_BTC_USD).into()],
        default_filter.clone(),
    );
    let just_pyth_btc_id = PriceIdentifier([0x02_u8; 32]);
    let just_redstone_eth = Proxy::median_low(
        [OracleRequest::redstone(redstone_adapter.clone(), "ETH").into()],
        default_filter.clone(),
    );
    let just_redstone_eth_id = PriceIdentifier([0x03_u8; 32]);

    harness
        .admin_set_proxy(
            proxy_oracle.clone(),
            btc_proxy_id,
            Some(btc_proxy_def.clone()),
        )
        .await?;
    harness
        .admin_set_proxy(
            proxy_oracle.clone(),
            just_pyth_btc_id,
            Some(just_pyth_btc.clone()),
        )
        .await?;
    harness
        .admin_set_proxy(
            proxy_oracle.clone(),
            just_redstone_eth_id,
            Some(just_redstone_eth.clone()),
        )
        .await?;

    let list_proxies: Vec<PriceIdentifier> = common::view(
        &network,
        &proxy_oracle,
        "list_proxies",
        json!({ "offset": null, "count": null }),
    )
    .await?;
    assert_eq!(
        list_proxies,
        vec![btc_proxy_id, just_pyth_btc_id, just_redstone_eth_id],
    );
    let stored_btc: Option<Proxy<Source>> = common::view(
        &network,
        &proxy_oracle,
        "get_proxy",
        json!({ "id": btc_proxy_id }),
    )
    .await?;
    assert_eq!(stored_btc.unwrap(), btc_proxy_def);

    let result: OracleResponse = common::view(
        &network,
        &proxy_oracle,
        "list_ema_prices_no_older_than",
        json!({ "price_ids": [btc_proxy_id, CRYPTO_BTC_USD], "age": 60 }),
    )
    .await?;
    assert_eq!(result, HashMap::from_iter([(btc_proxy_id, None)]));

    // Step 1: Only redstone has a price. Single source -> same for both methods.
    harness
        .set_mock_oracle_redstone_price(
            redstone_adapter.clone(),
            "BTC".into(),
            Some(common::redstone_price_now(100_000)),
        )
        .await?;
    let result = update_and_list(
        &network,
        &proxy_oracle,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60,
    )
    .await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(100_000),
    );

    // Step 2: redstone=100k, pyth1=90k.
    //   MedianLow: median of [90k, 100k] -> 90k
    //   Priority: redstone > pyth1 -> 100k
    harness
        .set_mock_oracle_pyth_price(
            pyth_oracle.clone(),
            CRYPTO_BTC_USD,
            Some(common::pyth_price_now(90_000)),
        )
        .await?;
    harness
        .set_mock_oracle_redstone_price(
            redstone_adapter.clone(),
            "ETH".into(),
            Some(common::redstone_price_now(1_800)),
        )
        .await?;
    let result = update_and_list(
        &network,
        &proxy_oracle,
        vec![
            btc_proxy_id,
            CRYPTO_BTC_USD,
            btc_proxy_id,
            CRYPTO_BTC_USD,
            CRYPTO_BTC_USD,
            just_pyth_btc_id,
            just_redstone_eth_id,
        ],
        60,
    )
    .await?;
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
    //   MedianLow: median of [80k, 90k, 100k] -> 90k
    //   Priority: pyth2 wins -> 80k
    harness
        .set_mock_oracle_pyth_price(
            pyth_oracle2.clone(),
            CRYPTO_BTC_USD,
            Some(common::pyth_price_now(80_000)),
        )
        .await?;
    let result = update_and_list(
        &network,
        &proxy_oracle,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60,
    )
    .await?;
    assert_eq!(result.len(), 1);
    let expected_btc_3source = match method {
        TestMethod::MedianLow => 90_000,
        TestMethod::Priority => 80_000,
    };
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(expected_btc_3source),
    );

    // Step 4: Clear pyth1 and redstone, only pyth2=80k remains.
    harness
        .set_mock_oracle_pyth_price(pyth_oracle.clone(), CRYPTO_BTC_USD, None)
        .await?;
    harness
        .set_mock_oracle_redstone_price(redstone_adapter.clone(), "BTC".into(), None)
        .await?;
    let result = update_and_list(
        &network,
        &proxy_oracle,
        vec![btc_proxy_id, CRYPTO_BTC_USD],
        60,
    )
    .await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(80_000),
    );

    Ok(())
}

#[rstest::rstest]
#[case::median_low(TestMethod::MedianLow, 100_000)]
#[case::priority(TestMethod::Priority, 100_000)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn proxy_oracle_enforces_freshness_filter(
    #[case] method: TestMethod,
    #[case] expected_price: u64,
) -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = harness.network.clone();

    let pyth_oracle = harness
        .deploy_mock_oracle("pyth-oracle.near".parse()?)
        .await?;
    let pyth_oracle2 = harness
        .deploy_mock_oracle("pyth-oracle2.near".parse()?)
        .await?;
    let redstone_adapter = harness
        .deploy_mock_oracle("redstone-adapter.near".parse()?)
        .await?;
    let proxy_oracle = harness.deploy_proxy_oracle().await?;

    let default_filter = FreshnessFilter::new(
        Some(Nanoseconds::from_secs(10)),
        Some(Nanoseconds::from_secs(10)),
    );

    let btc_proxy_def = match method {
        TestMethod::MedianLow => Proxy::new(
            Aggregator::MedianLow(MedianLow::new([
                WeightedSource::new(OracleRequest::pyth(pyth_oracle.clone(), CRYPTO_BTC_USD), 1),
                WeightedSource::new(OracleRequest::redstone(redstone_adapter.clone(), "BTC"), 1),
                WeightedSource::new(OracleRequest::pyth(pyth_oracle2.clone(), CRYPTO_BTC_USD), 1),
            ])),
            default_filter,
        ),
        TestMethod::Priority => Proxy::priority(
            [
                OracleRequest::pyth(pyth_oracle2.clone(), CRYPTO_BTC_USD).into(),
                OracleRequest::redstone(redstone_adapter.clone(), "BTC").into(),
                OracleRequest::pyth(pyth_oracle.clone(), CRYPTO_BTC_USD).into(),
            ],
            default_filter,
        ),
    };
    let btc_proxy_id = PriceIdentifier([0x09_u8; 32]);
    harness
        .admin_set_proxy(proxy_oracle.clone(), btc_proxy_id, Some(btc_proxy_def))
        .await?;

    let now = common::now_ns();
    let stale_time = now.saturating_sub(Nanoseconds::from_secs(30));
    let future_time = now.saturating_add(Nanoseconds::from_secs(20));

    harness
        .set_mock_oracle_pyth_price(
            pyth_oracle.clone(),
            CRYPTO_BTC_USD,
            Some(common::pyth_price_at(expected_price as i64, now)),
        )
        .await?;
    harness
        .set_mock_oracle_pyth_price(
            pyth_oracle2.clone(),
            CRYPTO_BTC_USD,
            Some(common::pyth_price_at(80_000, future_time)),
        )
        .await?;
    harness
        .set_mock_oracle_redstone_price(
            redstone_adapter.clone(),
            "BTC".into(),
            Some(common::redstone_price_at(90_000, stale_time)),
        )
        .await?;

    let result = update_and_list(&network, &proxy_oracle, vec![btc_proxy_id], 60).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.get(&btc_proxy_id).unwrap().as_ref().map(norm_price),
        Some(expected_price)
    );

    Ok(())
}
