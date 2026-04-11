#![allow(clippy::unwrap_used)]

use near_workspaces::{network::Sandbox, Worker};
use templar_common::time::Nanoseconds;
use templar_proxy_oracle_kernel::request::OracleRequest;
use templar_proxy_oracle_kernel::{
    proxy::{governance::Operation, Aggregator, FreshnessFilter, Source},
    state,
};
use test_utils::{
    assert_all_outcomes_success, controller::migration::MigrationController, worker,
    ContractController, GovernanceController, ProxyOracleController,
};

#[path = "support/migration_fixture.rs"]
mod fixture;

#[rstest::rstest]
#[tokio::test]
async fn new_account_writes_current_state_version_on_init(#[future(awt)] worker: Worker<Sandbox>) {
    let proxy = ProxyOracleController::deploy(worker.dev_create_account().await.unwrap()).await;

    assert_eq!(proxy.get_target_state_version().await, 1);
    assert_eq!(proxy.get_stored_state_version().await, 1);
    assert!(!proxy.needs_migration().await);
}

#[rstest::rstest]
#[tokio::test]
async fn migrate_accepts_v0_patch(#[future(awt)] worker: Worker<Sandbox>) {
    let proxy = fixture::deploy_patched(&worker).await;

    assert_eq!(proxy.get_stored_state_version().await, 0);
    assert_eq!(proxy.get_target_state_version().await, 1);
    assert!(proxy.needs_migration().await);

    let result = proxy
        .migrate(
            proxy.contract().as_account(),
            state::migration::Migration::from(state::migration::V0ToV1),
        )
        .await;

    assert_all_outcomes_success(&result);

    assert_eq!(proxy.get_stored_state_version().await, 1);
    assert_eq!(proxy.get_target_state_version().await, 1);
    assert!(!proxy.needs_migration().await);

    assert_eq!(proxy.gov_next_id().await, 6);
    assert_eq!(proxy.gov_ttl_ns().await, Nanoseconds::from_secs(30));

    let proxies = proxy.list_proxies(None, None).await;
    assert_eq!(
        proxies,
        vec![
            fixture::BTC_PRICE_ID,
            fixture::ETH_PRICE_ID,
            fixture::STNEAR_PRICE_ID,
        ],
    );

    let btc = proxy.get_proxy(fixture::BTC_PRICE_ID).await.unwrap();
    assert_eq!(
        btc.freshness_filter,
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(60)),
            Some(Nanoseconds::from_secs(10)),
        ),
    );
    match btc.aggregator {
        Aggregator::MedianLow(aggregator) => {
            assert_eq!(aggregator.min_sources, 2);
            assert_eq!(aggregator.sources.len(), 2);
            assert_eq!(aggregator.sources[0].weight, 3);
            assert_eq!(aggregator.sources[1].weight, 1);
        }
        other => panic!("unexpected aggregator: {other:?}"),
    }

    let eth = proxy.get_proxy(fixture::ETH_PRICE_ID).await.unwrap();
    assert_eq!(
        eth.freshness_filter,
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(70)),
            Some(Nanoseconds::from_secs(20)),
        ),
    );
    match eth.aggregator {
        Aggregator::Priority(priority) => {
            assert_eq!(priority.sources.len(), 3);
            assert!(matches!(
                priority.sources[0],
                Source::Request(OracleRequest::RedStone(_))
            ));
            assert!(matches!(
                priority.sources[1],
                Source::Request(OracleRequest::Pyth(_))
            ));
            assert!(matches!(
                priority.sources[2],
                Source::Request(OracleRequest::Pyth(_))
            ));
        }
        other => panic!("unexpected aggregator: {other:?}"),
    }

    let stnear = proxy.get_proxy(fixture::STNEAR_PRICE_ID).await.unwrap();
    match stnear.aggregator {
        Aggregator::MedianLow(aggregator) => {
            assert_eq!(aggregator.min_sources, 1);
            assert_eq!(aggregator.sources.len(), 2);
            assert!(matches!(
                aggregator.sources[0].source,
                Source::Transformer(_)
            ));
            assert!(matches!(aggregator.sources[1].source, Source::Request(_)));
        }
        other => panic!("unexpected aggregator: {other:?}"),
    }

    let pending = proxy.gov_get(4_u32).await.unwrap();
    assert_eq!(pending.ttl, Nanoseconds::from_secs(30));
    match pending.operation {
        Operation::SetProxy {
            id,
            proxy: Some(proxy),
        } => {
            assert_eq!(id, fixture::PENDING_PRICE_ID);
            assert_eq!(
                proxy.freshness_filter,
                FreshnessFilter::new(
                    Some(Nanoseconds::from_secs(30)),
                    Some(Nanoseconds::from_secs(5)),
                ),
            );
            match proxy.aggregator {
                Aggregator::Priority(priority) => {
                    assert_eq!(priority.sources.len(), 3);
                    assert!(matches!(
                        priority.sources[0],
                        Source::Request(OracleRequest::Pyth(_))
                    ));
                    assert!(matches!(
                        priority.sources[1],
                        Source::Request(OracleRequest::RedStone(_))
                    ));
                    assert!(matches!(
                        priority.sources[2],
                        Source::Request(OracleRequest::Pyth(_))
                    ));
                }
                other => panic!("unexpected aggregator: {other:?}"),
            }
        }
        other => panic!("unexpected operation: {other:?}"),
    }

    let pending_ttl = proxy.gov_get(5_u32).await.unwrap();
    assert!(matches!(
        pending_ttl.operation,
        Operation::SetActionTtl {
            new_ttl
        } if new_ttl == Nanoseconds::from_secs(90)
    ));
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "raw mainnet storage dump still needs normalization before replay"]
async fn migrate_accepts_mainnet_patch(#[future(awt)] worker: Worker<Sandbox>) {
    let proxy =
        fixture::deploy_patched_with_state_patch(&worker, fixture::load_mainnet_state_patch())
            .await;

    assert_eq!(proxy.get_stored_state_version().await, 0);
    assert_eq!(proxy.get_target_state_version().await, 1);
    assert!(proxy.needs_migration().await);

    let result = proxy
        .migrate(
            proxy.contract().as_account(),
            state::migration::Migration::from(state::migration::V0ToV1),
        )
        .await;

    assert_all_outcomes_success(&result);

    assert_eq!(proxy.get_stored_state_version().await, 1);
    assert_eq!(proxy.get_target_state_version().await, 1);
    assert!(!proxy.needs_migration().await);

    assert_eq!(proxy.gov_count().await, 0);

    let proxies = proxy.list_proxies(None, None).await;
    assert_eq!(proxies.len(), 2);

    for price_id in proxies {
        let proxy_def = proxy.get_proxy(price_id).await.unwrap();
        assert_eq!(
            proxy_def.freshness_filter,
            FreshnessFilter::new(
                Some(Nanoseconds::from_secs(60)),
                Some(Nanoseconds::from_secs(10)),
            )
        );
        assert!(!proxy_def.sources().is_empty());
    }
}

#[rstest::rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: migrate function is private"]
async fn migrate_is_private(#[future(awt)] worker: Worker<Sandbox>) {
    let proxy = fixture::deploy_patched(&worker).await;
    let caller = worker.dev_create_account().await.unwrap();

    caller
        .call(proxy.contract().id(), "migrate")
        .args_json(state::migration::Migration::from(state::migration::V0ToV1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();
}
