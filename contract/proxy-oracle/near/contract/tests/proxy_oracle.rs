#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::unwrap_used
)]

use std::collections::HashMap;
use std::str::FromStr;

use near_sdk::{
    json_types::{Base64VecU8, I64, U64},
    test_utils::VMContextBuilder,
    testing_env, AccountIdRef, Gas, NearToken,
};
use near_workspaces::{network::Sandbox, Account, Worker};

use templar_common::{
    governance::Proposal,
    oracle::{
        pyth::{self, PriceIdentifier, PythTimestamp},
        redstone::FeedData,
    },
    primitive_types, Decimal, Nanoseconds,
};
use templar_proxy_oracle_kernel::{
    primitive::AccountId as KernelAccountId,
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            CircuitBreaker, CircuitBreakerSetConfig, CircuitBreakerStatus, MonotonicRun,
            PriceBlockedReason, StepwiseChange, WindowedChangeDelta,
        },
        FreshnessFilter, Proxy, WeightedSource,
    },
    Price,
};
use templar_proxy_oracle_near_common::{
    cache::CachedProxyPriceStatus,
    governance::{
        AcceptedHistorySource, CircuitBreakerUpdate, Operation, ProxyGovernanceInterface,
        MAX_CIRCUIT_BREAKERS_PER_PROXY, MAX_CIRCUIT_BREAKER_HISTORY_LEN,
    },
    input::ProxyPriceTransformer,
    price_transformer,
    request::OracleRequest,
    role::Role,
};
use templar_proxy_oracle_near_contract::Contract;
use test_utils::{
    accounts,
    controller::proxy_oracle::ProxyOracleController,
    pyth_price_id::{self, stable::CRYPTO_BTC_USD},
    worker, ContractController, GovernanceController, MockOracleController,
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

fn joined_logs() -> String {
    near_sdk::test_utils::get_logs().join("\n")
}

fn kernel_actor_id() -> KernelAccountId {
    let mut bytes = [0; 64];
    let account_id = b"owner.near";
    bytes[..account_id.len()].copy_from_slice(account_id);
    KernelAccountId::from_bytes(bytes)
}

#[rstest::rstest]
#[tokio::test]
async fn proxy_oracle_circuit_breaker_trips_price_feed(#[future(awt)] worker: Worker<Sandbox>) {
    accounts!(worker, actor, proxy_oracle, pyth_oracle);
    let pyth_oracle = MockOracleController::deploy(pyth_oracle).await;
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle).await;

    let proxy_id = PriceIdentifier([0x44; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );
    proxy_oracle
        .set_proxy(proxy_oracle.account(), proxy_id, Some(proxy))
        .await;
    proxy_oracle
        .set_circuit_breaker_set_config(
            proxy_oracle.account(),
            proxy_id,
            CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::zero(),
                history_len: 2,
            },
        )
        .await;
    proxy_oracle
        .add_circuit_breaker(
            proxy_oracle.account(),
            proxy_id,
            0,
            CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: Decimal::from_str("0.10").unwrap(),
            }),
        )
        .await;

    pyth_oracle
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(100),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::from_secs(
                    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64,
                ),
            }),
        )
        .await;
    let result = update_and_list(&proxy_oracle, &actor, vec![proxy_id], 60_u32).await;
    assert_eq!(
        result.get(&proxy_id).unwrap().as_ref().map(norm_price),
        Some(100)
    );

    pyth_oracle
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(120),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::from_secs(
                    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64,
                ),
            }),
        )
        .await;
    let outcome = proxy_oracle
        .update_prices_exec(&actor, vec![proxy_id])
        .await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![proxy_id], 60_u32)
        .await;
    assert_eq!(result.get(&proxy_id), Some(&None));
    let logs = outcome.logs().join("\n");
    assert!(logs.contains("\"event\":\"circuit_breaker_tripped\""));
    assert!(logs.contains(
        "\"price_id\":\"4444444444444444444444444444444444444444444444444444444444444444\""
    ));
    assert!(logs.contains("\"breaker_id\":0"));
    assert!(logs.contains("\"is_enforced\":true"));

    let set = proxy_oracle
        .get_proxy_circuit_breaker_set(proxy_id)
        .await
        .unwrap();
    assert!(matches!(
        set.breakers().get(&0).unwrap().status,
        CircuitBreakerStatus::Tripped {
            price_update,
            ..
        } if price_update.price.price == 120
    ));

    pyth_oracle
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(130),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::from_secs(
                    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64,
                ),
            }),
        )
        .await;
    let outcome = proxy_oracle
        .update_prices_exec(&actor, vec![proxy_id])
        .await;
    let result = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![proxy_id], 60_u32)
        .await;
    assert_eq!(result.get(&proxy_id), Some(&None));
    assert!(!outcome
        .logs()
        .join("\n")
        .contains("\"event\":\"circuit_breaker_tripped\""));

    let set = proxy_oracle
        .get_proxy_circuit_breaker_set(proxy_id)
        .await
        .unwrap();
    assert_eq!(set.accepted_history().len(), 1);
    assert_eq!(set.accepted_history().as_slice()[0].price.price, 100);
    assert_eq!(set.observed_history().len(), 2);
    assert_eq!(set.observed_history().as_slice()[0].price.price, 120);
    assert_eq!(set.observed_history().as_slice()[1].price.price, 130);
}

#[rstest::rstest]
#[tokio::test]
async fn update_prices_does_not_use_read_age(#[future(awt)] worker: Worker<Sandbox>) {
    accounts!(worker, actor, proxy_oracle, pyth_oracle);
    let pyth_oracle = MockOracleController::deploy(pyth_oracle).await;
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle).await;

    let proxy_id = PriceIdentifier([0x55; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );
    proxy_oracle
        .set_proxy(proxy_oracle.account(), proxy_id, Some(proxy))
        .await;

    let publish_time = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64 - 120;
    pyth_oracle
        .set_pyth_price(
            &actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(100),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::from_secs(publish_time),
            }),
        )
        .await;

    proxy_oracle.update_prices(&actor, vec![proxy_id]).await;

    let strict_read = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![proxy_id], 60_u32)
        .await;
    assert_eq!(strict_read.get(&proxy_id), Some(&None));

    let loose_read = proxy_oracle
        .list_ema_prices_no_older_than(&actor, vec![proxy_id], 180_u32)
        .await;
    assert_eq!(
        loose_read.get(&proxy_id).unwrap().as_ref().map(norm_price),
        Some(100)
    );
}

#[derive(Clone, Copy, Debug)]
enum CalibrationBreakerKind {
    StepwiseChange,
    MonotonicRun,
    WindowedChangeDelta,
}

impl CalibrationBreakerKind {
    fn name(self) -> &'static str {
        match self {
            Self::StepwiseChange => "stepwise_change",
            Self::MonotonicRun => "monotonic_run",
            Self::WindowedChangeDelta => "windowed_change_delta",
        }
    }

    fn breaker(self, history_len: u32) -> CircuitBreaker {
        match self {
            Self::StepwiseChange => CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: Decimal::from_str("10").unwrap(),
            }),
            Self::MonotonicRun => CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak: history_len.saturating_add(2),
                min_relative_step_change: Decimal::ZERO,
            }),
            Self::WindowedChangeDelta => CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                window_len: 2,
                lookback_windows: 1,
                max_relative_change_delta: Decimal::from_str("10").unwrap(),
            }),
        }
    }
}

async fn set_calibration_price(
    pyth_oracle: &MockOracleController,
    actor: &near_workspaces::Account,
    value: i64,
) {
    pyth_oracle
        .set_pyth_price(
            actor,
            CRYPTO_BTC_USD,
            Some(pyth::Price {
                price: I64(value),
                conf: U64(0),
                expo: 0,
                publish_time: PythTimestamp::from_secs(
                    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs() as i64,
                ),
            }),
        )
        .await;
}

async fn execute_governance_operation(
    proxy_oracle: &ProxyOracleController,
    operation: Operation,
) -> near_workspaces::result::ExecutionSuccess {
    let proposal_id = proxy_oracle.gov_next_id().await;
    proxy_oracle
        .account()
        .call(proxy_oracle.id(), "gov_create")
        .args_json(near_sdk::serde_json::json!({ "id": proposal_id, "operation": operation }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();

    proxy_oracle
        .account()
        .call(proxy_oracle.id(), "gov_execute")
        .args_json(near_sdk::serde_json::json!({ "id": proposal_id }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap()
}

fn max_receipt_gas_burnt(result: &near_workspaces::result::ExecutionSuccess) -> near_sdk::Gas {
    result
        .receipt_outcomes()
        .iter()
        .map(|outcome| outcome.gas_burnt)
        .max()
        .unwrap_or_default()
}

fn executor_receipt_gas_burnt(
    result: &near_workspaces::result::ExecutionSuccess,
    executor_id: &near_sdk::AccountId,
) -> near_sdk::Gas {
    result
        .receipt_outcomes()
        .iter()
        .filter(|outcome| outcome.executor_id == *executor_id)
        .map(|outcome| outcome.gas_burnt)
        .max()
        .unwrap_or_default()
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "prints gas for choosing circuit breaker history and breaker-count bounds"]
async fn calibrate_circuit_breaker_resolution_gas(#[future(awt)] worker: Worker<Sandbox>) {
    const CASES: &[(CalibrationBreakerKind, u32, u32)] = &[
        (CalibrationBreakerKind::StepwiseChange, 0, 0),
        (CalibrationBreakerKind::StepwiseChange, 8, 16),
        (CalibrationBreakerKind::MonotonicRun, 8, 16),
        (CalibrationBreakerKind::MonotonicRun, 32, 16),
        (CalibrationBreakerKind::WindowedChangeDelta, 8, 16),
        (CalibrationBreakerKind::WindowedChangeDelta, 32, 16),
    ];

    accounts!(worker, actor, proxy_oracle, pyth_oracle);
    let pyth_oracle = MockOracleController::deploy(pyth_oracle).await;
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle).await;
    let mut case_index = 0_u8;

    eprintln!("rule,history_len,breaker_count,total_gas_burnt,max_receipt_gas_burnt,proxy_oracle_receipt_gas_burnt");
    for &(kind, history_len, breaker_count) in CASES {
        case_index = case_index.wrapping_add(1);
        let proxy_id = PriceIdentifier([case_index; 32]);
        let proxy = Proxy::median_low(
            [OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()],
            FreshnessFilter::empty(),
        );
        proxy_oracle
            .set_proxy(proxy_oracle.account(), proxy_id, Some(proxy))
            .await;
        proxy_oracle
            .set_circuit_breaker_set_config(
                proxy_oracle.account(),
                proxy_id,
                CircuitBreakerSetConfig {
                    sample_interval_ns: Nanoseconds::zero(),
                    history_len,
                },
            )
            .await;

        for breaker_id in 0..breaker_count {
            execute_governance_operation(
                &proxy_oracle,
                Operation::AddCircuitBreaker {
                    id: proxy_id,
                    breaker_id,
                    breaker: kind.breaker(history_len),
                },
            )
            .await;
        }

        for value in 0..history_len {
            set_calibration_price(&pyth_oracle, &actor, i64::from(100 + value)).await;
            proxy_oracle.update_prices(&actor, vec![proxy_id]).await;
        }

        set_calibration_price(&pyth_oracle, &actor, 1_000).await;
        let result = actor
            .call(proxy_oracle.id(), "update_prices")
            .args_json(near_sdk::serde_json::json!({
                "price_ids": [proxy_id],
            }))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "{},{history_len},{breaker_count},{},{},{}",
            kind.name(),
            result.total_gas_burnt,
            max_receipt_gas_burnt(&result),
            executor_receipt_gas_burnt(&result, proxy_oracle.id())
        );
    }
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "prints governance gas for configuring and adding circuit breakers"]
async fn calibrate_circuit_breaker_governance_gas(#[future(awt)] worker: Worker<Sandbox>) {
    accounts!(worker, proxy_oracle, pyth_oracle);
    let proxy_oracle = ProxyOracleController::deploy(proxy_oracle).await;
    let proxy_id = PriceIdentifier([0x77; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth(pyth_oracle.id().clone(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );

    execute_governance_operation(
        &proxy_oracle,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    )
    .await;

    eprintln!("operation,history_len,breaker_id,total_gas_burnt");
    for history_len in [0_u32, 2, 8, MAX_CIRCUIT_BREAKER_HISTORY_LEN] {
        let result = execute_governance_operation(
            &proxy_oracle,
            Operation::ConfigureCircuitBreakers {
                id: proxy_id,
                config: CircuitBreakerSetConfig {
                    sample_interval_ns: Nanoseconds::zero(),
                    history_len,
                },
            },
        )
        .await;
        eprintln!("configure,{history_len},,{}", result.total_gas_burnt);
    }

    for breaker_id in 0_u32..u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap() {
        let result = execute_governance_operation(
            &proxy_oracle,
            Operation::AddCircuitBreaker {
                id: proxy_id,
                breaker_id,
                breaker: CalibrationBreakerKind::StepwiseChange
                    .breaker(MAX_CIRCUIT_BREAKER_HISTORY_LEN),
            },
        )
        .await;
        eprintln!("add,,{breaker_id},{}", result.total_gas_burnt);
    }
}

fn estimate_gas(c: &Contract, price_ids: &[PriceIdentifier]) -> near_sdk::Gas {
    let _ = (c, price_ids);
    Contract::GAS_FOR_LIST_00_ENTRY
}

fn test_proxy(oracle_id: &str) -> Proxy<templar_proxy_oracle_near_common::input::Source> {
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

#[allow(clippy::unwrap_used)]
#[test]
pub fn governance_manual_trip_invalidates_cached_price() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("owner.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x56; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));

    let initial_epoch = c.cache_epoch(proxy_id);
    assert!(c.get_cached_proxy_price(proxy_id).is_some());

    c.gov_create(
        0,
        Operation::SetCircuitBreakerManualTrip {
            id: proxy_id,
            is_manually_tripped: true,
        },
    );
    c.gov_execute(0);

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
pub fn governance_breaker_update_invalidates_cached_price() {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id("owner.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build());
    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x57; 32]);
    c.set_proxy(proxy_id, Some(test_proxy("pyth-oracle.near")));
    cache_test_price(&mut c, proxy_id, proxy_price(100));

    let initial_epoch = c.cache_epoch(proxy_id);
    c.gov_create(
        0,
        Operation::ConfigureCircuitBreakers {
            id: proxy_id,
            config: CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::zero(),
                history_len: 3,
            },
        },
    );
    c.gov_execute(0);

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
        c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| {
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
    c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| status)
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
#[case::success(10 * 1000)]
#[should_panic = "TTL not yet elapsed for proposal"]
#[case::fail(0)]
#[should_panic = "TTL not yet elapsed for proposal"]
#[case::fail(10 * 1000 - 1)]
pub fn governance_ttl(#[case] delay_ms: u64) {
    let mut context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(1_000_000)
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();

    assert_eq!(c.gov_count(), 0);
    assert_eq!(c.gov_next_id(), 0);
    assert_eq!(c.gov_get(0), None);
    assert_eq!(c.gov_list(None, None), Vec::<u32>::new());
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::zero());

    let proposal = c.gov_create(
        0,
        Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(10),
        },
    );

    let expected = Proposal {
        operation: Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(10),
        },
        ttl: Nanoseconds::zero(),
        created_at: Nanoseconds::from_ms(1),
        created_by: "owner.near".parse().unwrap(),
    };

    assert_eq!(proposal, expected);
    assert_eq!(c.gov_get(0).unwrap(), expected);
    assert_eq!(c.gov_list(Some(0), Some(1)), vec![0]);
    assert_eq!(c.gov_list(None, None), vec![0]);
    assert_eq!(c.gov_count(), 1);
    assert_eq!(c.gov_next_id(), 1);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::zero());

    c.gov_execute(0);
    assert_eq!(c.gov_get(0), None);
    assert_eq!(c.gov_list(Some(0), Some(1)), Vec::<u32>::new());
    assert_eq!(c.gov_list(None, None), Vec::<u32>::new());
    assert_eq!(c.gov_count(), 0);
    assert_eq!(c.gov_next_id(), 1);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::from_secs(10));

    let proxy_id = PriceIdentifier([0x01_u8; 32]);
    let proxy_def = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );

    let proposal = c.gov_create(
        1,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy_def.clone()),
        },
    );
    let expected = Proposal {
        operation: Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy_def),
        },
        ttl: Nanoseconds::from_secs(10),
        created_at: Nanoseconds::from_ms(1),
        created_by: "owner.near".parse().unwrap(),
    };
    assert_eq!(proposal, expected);
    assert_eq!(c.gov_get(1).unwrap(), expected);
    assert_eq!(c.gov_list(Some(0), Some(2)), vec![1]);
    assert_eq!(c.gov_list(None, None), vec![1]);
    assert_eq!(c.gov_count(), 1);
    assert_eq!(c.gov_next_id(), 2);
    assert_eq!(c.gov_ttl_ns(), Nanoseconds::from_secs(10));

    context.block_timestamp += delay_ms * 1_000_000;
    testing_env!(context.clone());

    c.gov_execute(1);
}

#[test]
#[should_panic = "Empty proxy definition is not allowed"]
fn governance_rejects_empty_proxy_definition_on_create() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();
    c.gov_create(
        0,
        Operation::SetProxy {
            id: PriceIdentifier([0xFF; 32]),
            proxy: Some(Proxy::median_low([], FreshnessFilter::empty())),
        },
    );
}

#[test]
fn governance_sets_circuit_breaker_roles() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();
    let account_id: near_sdk::AccountId = "offline-breaker.near".parse().unwrap();

    assert!(!c.has_role(account_id.clone(), Role::OfflineManualTrip));
    assert!(c.list_role(Role::OfflineManualTrip, None, None).is_empty());

    c.gov_create(
        0,
        Operation::SetCircuitBreakerRole {
            account_id: account_id.clone(),
            role: Role::OfflineManualTrip,
            is_granted: true,
        },
    );
    c.gov_execute(0);

    assert!(c.has_role(account_id.clone(), Role::OfflineManualTrip));
    assert!(!c.has_role(account_id.clone(), Role::OfflineManualUntrip));
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_role_set\""));
    assert!(logs.contains("\"account_id\":\"offline-breaker.near\""));
    assert!(logs.contains("\"role\":\"OfflineManualTrip\""));
    assert!(logs.contains("\"is_granted\":true"));
    assert_eq!(
        c.list_role(Role::OfflineManualTrip, None, None),
        vec![account_id.clone()]
    );

    c.gov_create(
        1,
        Operation::SetCircuitBreakerRole {
            account_id: account_id.clone(),
            role: Role::OfflineManualUntrip,
            is_granted: true,
        },
    );
    c.gov_execute(1);

    assert!(c.has_role(account_id.clone(), Role::OfflineManualTrip));
    assert!(c.has_role(account_id.clone(), Role::OfflineManualUntrip));
    let logs = joined_logs();
    assert!(logs.contains("\"role\":\"OfflineManualUntrip\""));
    assert!(logs.contains("\"is_granted\":true"));

    c.gov_create(
        2,
        Operation::SetCircuitBreakerRole {
            account_id: account_id.clone(),
            role: Role::OfflineManualTrip,
            is_granted: false,
        },
    );
    c.gov_execute(2);

    assert!(!c.has_role(account_id.clone(), Role::OfflineManualTrip));
    assert!(c.has_role(account_id, Role::OfflineManualUntrip));
    let logs = joined_logs();
    assert!(logs.contains("\"role\":\"OfflineManualTrip\""));
    assert!(logs.contains("\"is_granted\":false"));
    assert!(c.list_role(Role::OfflineManualTrip, None, None).is_empty());
}

#[test]
fn offline_manual_trip_roles_gate_trip_and_untrip() {
    let mut context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x46; 32]);
    let tripper: near_sdk::AccountId = "tripper.near".parse().unwrap();
    let untripper: near_sdk::AccountId = "untripper.near".parse().unwrap();
    let unauthorized: near_sdk::AccountId = "unauthorized.near".parse().unwrap();
    let proxy = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );

    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    );
    c.gov_execute(0);
    c.gov_create(
        1,
        Operation::SetCircuitBreakerRole {
            account_id: tripper.clone(),
            role: Role::OfflineManualTrip,
            is_granted: true,
        },
    );
    c.gov_execute(1);
    c.gov_create(
        2,
        Operation::SetCircuitBreakerRole {
            account_id: untripper.clone(),
            role: Role::OfflineManualUntrip,
            is_granted: true,
        },
    );
    c.gov_execute(2);

    context.predecessor_account_id = unauthorized;
    testing_env!(context.clone());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.set_circuit_breaker_manual_trip(proxy_id, true, None);
    }))
    .is_err());

    cache_test_price(&mut c, proxy_id, proxy_price(100));
    let initial_epoch = c.cache_epoch(proxy_id);

    context.predecessor_account_id = tripper;
    testing_env!(context.clone());
    c.set_circuit_breaker_manual_trip(proxy_id, true, Some(Base64VecU8(vec![1, 2, 3])));
    assert!(c.get_cached_proxy_price(proxy_id).is_none());
    assert!(c.cache_epoch(proxy_id) > initial_epoch);
    assert!(c
        .get_proxy_circuit_breaker_set(proxy_id)
        .unwrap()
        .is_manually_tripped());
    let logs = near_sdk::test_utils::get_logs();
    let joined = logs.join("\n");
    assert!(joined.contains("\"event\":\"circuit_breaker_manual_trip_set\""));
    assert!(joined.contains(
        "\"price_id\":\"4646464646464646464646464646464646464646464646464646464646464646\""
    ));
    assert!(joined.contains("\"is_manually_tripped\":true"));
    assert!(joined.contains("\"actor\":\"tripper.near\""));
    assert!(joined.contains("\"metadata\":\"AQID\""));

    let log_count = logs.len();
    let manual_trip_event_count = joined
        .matches("\"event\":\"circuit_breaker_manual_trip_set\"")
        .count();
    c.set_circuit_breaker_manual_trip(proxy_id, true, Some(Base64VecU8(vec![9])));
    assert_eq!(near_sdk::test_utils::get_logs().len(), log_count);
    assert_eq!(
        joined_logs()
            .matches("\"event\":\"circuit_breaker_manual_trip_set\"")
            .count(),
        manual_trip_event_count
    );

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.set_circuit_breaker_manual_trip(proxy_id, false, None);
    }))
    .is_err());

    context.predecessor_account_id = untripper;
    testing_env!(context);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.set_circuit_breaker_manual_trip(proxy_id, true, None);
    }))
    .is_err());
    c.set_circuit_breaker_manual_trip(proxy_id, false, None);
    assert!(!c
        .get_proxy_circuit_breaker_set(proxy_id)
        .unwrap()
        .is_manually_tripped());
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_manual_trip_set\""));
    assert!(logs.contains(
        "\"price_id\":\"4646464646464646464646464646464646464646464646464646464646464646\""
    ));
    assert!(logs.contains("\"is_manually_tripped\":false"));
    assert!(logs.contains("\"actor\":\"untripper.near\""));
    assert!(logs.contains("\"metadata\":null"));
}

#[test]
fn governance_emits_circuit_breaker_configuration_events() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context);

    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x48; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::from_str("0.10").unwrap(),
    });

    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    );
    c.gov_execute(0);

    c.gov_create(
        1,
        Operation::ConfigureCircuitBreakers {
            id: proxy_id,
            config: CircuitBreakerSetConfig {
                sample_interval_ns: Nanoseconds::from_secs(1),
                history_len: 3,
            },
        },
    );
    c.gov_execute(1);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_config_set\""));
    assert!(logs.contains(
        "\"price_id\":\"4848484848484848484848484848484848484848484848484848484848484848\""
    ));
    assert!(logs.contains("\"sample_interval_ns\":\"1000000000\""));
    assert!(logs.contains("\"history_len\":3"));

    c.gov_create(
        2,
        Operation::AddCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
            breaker,
        },
    );
    c.gov_execute(2);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_added\""));
    assert!(logs.contains("\"breaker_id\":0"));
    assert!(logs.contains("\"StepwiseChange\""));

    c.gov_create(
        3,
        Operation::SetCircuitBreakerManualTrip {
            id: proxy_id,
            is_manually_tripped: true,
        },
    );
    c.gov_execute(3);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_manual_trip_set\""));
    assert!(logs.contains("\"is_manually_tripped\":true"));
    assert!(logs.contains("\"actor\":\"owner.near\""));
    assert!(logs.contains("\"metadata\":null"));
    let manual_trip_event_count = logs
        .matches("\"event\":\"circuit_breaker_manual_trip_set\"")
        .count();

    c.gov_create(
        4,
        Operation::SetCircuitBreakerManualTrip {
            id: proxy_id,
            is_manually_tripped: true,
        },
    );
    c.gov_execute(4);
    assert_eq!(
        joined_logs()
            .matches("\"event\":\"circuit_breaker_manual_trip_set\"")
            .count(),
        manual_trip_event_count
    );

    c.gov_create(
        5,
        Operation::RemoveCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
        },
    );
    c.gov_execute(5);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_removed\""));
    assert!(logs.contains("\"breaker_id\":0"));
}

#[test]
#[should_panic = "Manual trip metadata is too long"]
fn offline_manual_trip_rejects_oversized_metadata() {
    let mut context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x47; 32]);
    let tripper: near_sdk::AccountId = "tripper.near".parse().unwrap();
    let proxy = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );

    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    );
    c.gov_execute(0);
    c.gov_create(
        1,
        Operation::SetCircuitBreakerRole {
            account_id: tripper.clone(),
            role: Role::OfflineManualTrip,
            is_granted: true,
        },
    );
    c.gov_execute(1);

    context.predecessor_account_id = tripper;
    testing_env!(context);
    c.set_circuit_breaker_manual_trip(proxy_id, true, Some(Base64VecU8(vec![0; 1025])));
}

#[test]
#[should_panic = "too many circuit breakers"]
fn governance_rejects_too_many_circuit_breakers_on_execute() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x44; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );
    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    );
    c.gov_execute(0);

    let breaker = || {
        CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: Decimal::from_str("0.10").unwrap(),
        })
    };

    for breaker_id in 0..u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap() {
        let proposal_id = breaker_id + 1;
        c.gov_create(
            proposal_id,
            Operation::AddCircuitBreaker {
                id: proxy_id,
                breaker_id,
                breaker: breaker(),
            },
        );
        c.gov_execute(proposal_id);
        testing_env!(context.clone());
    }

    let proposal_id = u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap() + 1;
    c.gov_create(
        proposal_id,
        Operation::AddCircuitBreaker {
            id: proxy_id,
            breaker_id: u32::try_from(MAX_CIRCUIT_BREAKERS_PER_PROXY).unwrap(),
            breaker: breaker(),
        },
    );
    c.gov_execute(proposal_id);
}

#[test]
fn governance_updates_circuit_breaker_enforcement_and_lifecycle_separately() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .predecessor_account_id("owner.near".parse().unwrap())
        .build();
    testing_env!(context);

    let mut c = Contract::new();
    let proxy_id = PriceIdentifier([0x45; 32]);
    let proxy = Proxy::median_low(
        [OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into()],
        FreshnessFilter::empty(),
    );
    c.gov_create(
        0,
        Operation::SetProxy {
            id: proxy_id,
            proxy: Some(proxy),
        },
    );
    c.gov_execute(0);
    c.gov_create(
        1,
        Operation::AddCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
            breaker: CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: Decimal::from_str("0.10").unwrap(),
            }),
        },
    );
    c.gov_execute(1);

    let pending = c.proxy_entry(proxy_id).unwrap().prepare_price_update();
    c.finish_price_update_if_current(pending, Nanoseconds::from_secs(2), |_, set| {
        set.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 3,
        });
        set.try_accept_price(proxy_price(100), Nanoseconds::from_secs(1))
            .unwrap();
        set.set_manual_trip(true, kernel_actor_id(), None);
        assert!(!set
            .try_accept_price(proxy_price(200), Nanoseconds::from_secs(2))
            .unwrap()
            .value
            .is_ok());
        set.set_manual_trip(false, kernel_actor_id(), None);
        CachedProxyPriceStatus::Accepted {
            price: proxy_price(200),
        }
    })
    .unwrap();

    c.gov_create(
        2,
        Operation::UpdateCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
            update: CircuitBreakerUpdate::SetEnforced { is_enforced: false },
        },
    );
    c.gov_execute(2);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_enforcement_set\""));
    assert!(logs.contains(
        "\"price_id\":\"4545454545454545454545454545454545454545454545454545454545454545\""
    ));
    assert!(logs.contains("\"breaker_id\":0"));
    assert!(logs.contains("\"is_enforced\":false"));
    let set = c.get_proxy_circuit_breaker_set(proxy_id).unwrap();
    let breaker = set.breakers().get(&0).unwrap();
    assert!(!breaker.is_enforced);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::ArmedAfter {
            timestamp_ns
        } if timestamp_ns == Nanoseconds::zero()
    ));

    c.gov_create(
        3,
        Operation::UpdateCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
            update: CircuitBreakerUpdate::Rearm {
                armed_after_ns: Nanoseconds::from_secs(1),
                accepted_history_source: AcceptedHistorySource::Empty,
            },
        },
    );
    c.gov_execute(3);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_rearmed\""));
    assert!(logs.contains("\"armed_after_ns\":\"1000000000\""));
    assert!(logs.contains("\"accepted_history_source\":\"Empty\""));
    let set = c.get_proxy_circuit_breaker_set(proxy_id).unwrap();
    let breaker = set.breakers().get(&0).unwrap();
    assert_eq!(set.accepted_history().len(), 0);
    assert!(!breaker.is_enforced);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::ArmedAfter {
            timestamp_ns
        } if timestamp_ns == Nanoseconds::from_secs(1)
    ));

    c.gov_create(
        4,
        Operation::UpdateCircuitBreaker {
            id: proxy_id,
            breaker_id: 0,
            update: CircuitBreakerUpdate::Rearm {
                armed_after_ns: Nanoseconds::from_secs(2),
                accepted_history_source: AcceptedHistorySource::Observed,
            },
        },
    );
    c.gov_execute(4);
    let logs = joined_logs();
    assert!(logs.contains("\"event\":\"circuit_breaker_rearmed\""));
    assert!(logs.contains("\"armed_after_ns\":\"2000000000\""));
    assert!(logs.contains("\"accepted_history_source\":\"Observed\""));
    let set = c.get_proxy_circuit_breaker_set(proxy_id).unwrap();
    let breaker = set.breakers().get(&0).unwrap();
    assert_eq!(set.accepted_history().len(), 2);
    assert_eq!(set.accepted_history().as_slice()[0].price.price, 100);
    assert_eq!(set.accepted_history().as_slice()[1].price.price, 200);
    assert!(!breaker.is_enforced);
    assert!(matches!(
        breaker.status,
        CircuitBreakerStatus::ArmedAfter {
            timestamp_ns
        } if timestamp_ns == Nanoseconds::from_secs(2)
    ));
}

#[allow(clippy::unwrap_used)]
#[test]
pub fn gas() {
    let context = VMContextBuilder::new()
        .attached_deposit(NearToken::from_yoctonear(1))
        .build();
    testing_env!(context.clone());

    let mut c = Contract::new();

    let proxy_btc = Proxy::median_low(
        [
            OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_BTC_USD).into(),
            OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "BTC").into(),
        ],
        FreshnessFilter::empty(),
    );
    let proxy_btc_id = PriceIdentifier([0x01_u8; 32]);

    let proxy_usdc = Proxy::median_low(
        [
            OracleRequest::pyth(
                "pyth-oracle.near".parse().unwrap(),
                pyth_price_id::stable::CRYPTO_USDC_USD,
            )
            .into(),
            OracleRequest::redstone("redstone-adapter.near".parse().unwrap(), "USDC").into(),
        ],
        FreshnessFilter::empty(),
    );
    let proxy_usdc_id = PriceIdentifier([0x02_u8; 32]);

    let proxy_wnear = Proxy::median_low(
        [ProxyPriceTransformer::lst(
            OracleRequest::pyth(
                "pyth-oracle.near".parse().unwrap(),
                pyth_price_id::stable::CRYPTO_NEAR_USD,
            ),
            24,
            price_transformer::Call::new_simple(
                AccountIdRef::new_or_panic("wrap.near"),
                "redemption_rate",
            ),
        )
        .into()],
        FreshnessFilter::empty(),
    );
    let proxy_wnear_id = PriceIdentifier([0x03_u8; 32]);

    let price_ids = vec![proxy_btc_id, proxy_usdc_id, proxy_wnear_id];

    let mut set_proxy = |id, price_id, proxy| {
        c.gov_create(
            id,
            Operation::SetProxy {
                id: price_id,
                proxy: Some(proxy),
            },
        );
        c.gov_execute(id);
    };

    set_proxy(0, proxy_btc_id, proxy_btc.clone());
    set_proxy(1, proxy_usdc_id, proxy_usdc.clone());
    set_proxy(2, proxy_wnear_id, proxy_wnear.clone());

    for (index, price_id) in price_ids.iter().copied().enumerate() {
        let pending = c.proxy_entry(price_id).unwrap().prepare_price_update();
        c.finish_price_update_if_current(pending, Nanoseconds::zero(), |_, _| {
            CachedProxyPriceStatus::Accepted {
                price: proxy_price(i64::try_from(index + 1).unwrap()),
            }
        })
        .unwrap();
    }

    let gas = estimate_gas(&c, &price_ids);
    eprintln!("Gas used: {gas}");
    assert!(gas <= Gas::from_tgas(15));

    let result = c.list_ema_prices_no_older_than(price_ids, 60);
    assert_eq!(result.len(), 3);
    assert!(near_sdk::test_utils::get_created_receipts().is_empty());
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
        .set_proxy(
            proxy_oracle.account(),
            btc_proxy_id,
            Some(btc_proxy_def.clone()),
        )
        .await;
    proxy_oracle
        .set_proxy(
            proxy_oracle.account(),
            just_pyth_btc_id,
            Some(just_pyth_btc.clone()),
        )
        .await;
    proxy_oracle
        .set_proxy(
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
        .set_proxy(
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
