#![allow(clippy::unwrap_used)]

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::{
    borsh, json_types::Base64VecU8, mock::with_mocked_blockchain, test_utils::VMContextBuilder,
    testing_env,
};
use near_token::NearToken;
use serde_json::json;
use templar_common::{
    oracle::pyth::PriceIdentifier, versioned_state::write_state_version, Nanoseconds,
};
use templar_gateway_testing::SandboxHarness;
use templar_proxy_oracle_kernel::proxy::{
    aggregator::method::median::MedianLow, Aggregator, FreshnessFilter, Proxy, WeightedSource,
};
use templar_proxy_oracle_near_common::{
    input::{ProxyPriceTransformer, Source},
    price_transformer::{Action, Call},
    request::OracleRequest,
    state,
    state::legacy::v0,
};

use common::StatePatch;

const BTC_PRICE_ID: PriceIdentifier = PriceIdentifier([0x41; 32]);
const ETH_PRICE_ID: PriceIdentifier = PriceIdentifier([0x42; 32]);
const STNEAR_PRICE_ID: PriceIdentifier = PriceIdentifier([0x43; 32]);
const PENDING_PRICE_ID: PriceIdentifier = PriceIdentifier([0x44; 32]);
const PROXY_ORACLE_ACCOUNT_ID: &str = "proxy-oracle.test.near";
const PROPOSAL_4_CREATED_AT: Nanoseconds = Nanoseconds::from_ns(1);
const PROPOSAL_5_CREATED_AT: Nanoseconds = Nanoseconds::from_ns(2);

fn patch_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migration/v0_state_patch.borsh")
}

fn patch() -> StatePatch {
    borsh::from_slice(include_bytes!("./migration/v0_state_patch.borsh")).unwrap()
}

fn executed_btc_proxy() -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator::median_low(v0::Filter {
            max_age: Some(Nanoseconds::from_secs(60)),
            max_clock_drift: Some(Nanoseconds::from_secs(10)),
            min_sources: Some(2),
        }),
        entries: vec![
            v0::Entry::new(
                OracleRequest::pyth("pyth.test.near".parse().unwrap(), BTC_PRICE_ID),
                3,
            ),
            v0::Entry::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "BTC"),
                1,
            ),
        ],
    }
}

fn executed_eth_proxy() -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator::priority(v0::Filter {
            max_age: Some(Nanoseconds::from_secs(70)),
            max_clock_drift: Some(Nanoseconds::from_secs(20)),
            min_sources: Some(1),
        }),
        entries: vec![
            v0::Entry::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "ETH"),
                7,
            ),
            v0::Entry::new(
                OracleRequest::pyth("pyth.test.near".parse().unwrap(), ETH_PRICE_ID),
                7,
            ),
            v0::Entry::new(
                OracleRequest::pyth("pyth2.test.near".parse().unwrap(), ETH_PRICE_ID),
                3,
            ),
        ],
    }
}

fn executed_stnear_proxy() -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator::median_low(v0::Filter {
            max_age: Some(Nanoseconds::from_secs(120)),
            max_clock_drift: Some(Nanoseconds::from_secs(15)),
            min_sources: Some(1),
        }),
        entries: vec![
            v0::Entry::new(
                v0::ProxyPriceTransformer {
                    request: OracleRequest::pyth(
                        "pyth.test.near".parse().unwrap(),
                        STNEAR_PRICE_ID,
                    ),
                    call: Call {
                        account_id: "wrap.near".parse().unwrap(),
                        method_name: "redemption_rate".to_string(),
                        args: Base64VecU8(b"null".to_vec()),
                        gas: near_sdk::Gas::from_tgas(3).as_gas().into(),
                    },
                    action: Action::NormalizeNativeLstPrice { decimals: 24 },
                },
                2,
            ),
            v0::Entry::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "stNEAR"),
                1,
            ),
        ],
    }
}

fn pending_proxy() -> v0::Proxy {
    v0::Proxy {
        aggregator: v0::Aggregator::priority(v0::Filter {
            max_age: Some(Nanoseconds::from_secs(30)),
            max_clock_drift: Some(Nanoseconds::from_secs(5)),
            min_sources: Some(1),
        }),
        entries: vec![
            v0::Entry::new(
                OracleRequest::pyth("pyth2.test.near".parse().unwrap(), PENDING_PRICE_ID),
                11,
            ),
            v0::Entry::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "PENDING"),
                9,
            ),
            v0::Entry::new(
                OracleRequest::pyth("pyth.test.near".parse().unwrap(), PENDING_PRICE_ID),
                9,
            ),
        ],
    }
}

fn expected_btc_proxy() -> Proxy<Source> {
    let mut proxy = Proxy::new(
        Aggregator::MedianLow(MedianLow::new([
            WeightedSource::new(
                OracleRequest::pyth("pyth.test.near".parse().unwrap(), BTC_PRICE_ID),
                3,
            ),
            WeightedSource::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "BTC"),
                1,
            ),
        ])),
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(60)),
            Some(Nanoseconds::from_secs(10)),
        ),
    );
    match &mut proxy.aggregator {
        Aggregator::MedianLow(aggregator) => aggregator.min_sources = 2,
        other => panic!("unexpected aggregator: {other:?}"),
    }
    proxy
}

fn expected_eth_proxy() -> Proxy<Source> {
    Proxy::priority(
        [
            OracleRequest::redstone("redstone.test.near".parse().unwrap(), "ETH").into(),
            OracleRequest::pyth("pyth.test.near".parse().unwrap(), ETH_PRICE_ID).into(),
            OracleRequest::pyth("pyth2.test.near".parse().unwrap(), ETH_PRICE_ID).into(),
        ],
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(70)),
            Some(Nanoseconds::from_secs(20)),
        ),
    )
}

fn expected_stnear_proxy() -> Proxy<Source> {
    let mut proxy = Proxy::new(
        Aggregator::MedianLow(MedianLow::new([
            WeightedSource::new(
                Source::Transformer(ProxyPriceTransformer {
                    request: OracleRequest::pyth(
                        "pyth.test.near".parse().unwrap(),
                        STNEAR_PRICE_ID,
                    ),
                    call: Call {
                        account_id: "wrap.near".parse().unwrap(),
                        method_name: "redemption_rate".to_string(),
                        args: Base64VecU8(b"null".to_vec()),
                        gas: near_sdk::Gas::from_tgas(3).as_gas().into(),
                    },
                    action: Action::NormalizeNativeLstPrice { decimals: 24 },
                }),
                2,
            ),
            WeightedSource::new(
                OracleRequest::redstone("redstone.test.near".parse().unwrap(), "stNEAR"),
                1,
            ),
        ])),
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(120)),
            Some(Nanoseconds::from_secs(15)),
        ),
    );
    match &mut proxy.aggregator {
        Aggregator::MedianLow(aggregator) => aggregator.min_sources = 1,
        other => panic!("unexpected aggregator: {other:?}"),
    }
    proxy
}

fn build_patch() -> StatePatch {
    testing_env!(VMContextBuilder::new().build());

    let mut state = v0::State {
        governance: v0::Governance {
            next_id: 6,
            ttl: Nanoseconds::from_secs(30),
            proposals: near_sdk::store::IterableMap::with_hasher(v0::StorageKey::Governance),
        },
        proxies: near_sdk::collections::UnorderedMap::new(v0::StorageKey::Proxies),
    };
    state.proxies.insert(&BTC_PRICE_ID, &executed_btc_proxy());
    state.proxies.insert(&ETH_PRICE_ID, &executed_eth_proxy());
    state
        .proxies
        .insert(&STNEAR_PRICE_ID, &executed_stnear_proxy());
    state.governance.proposals.insert(
        4,
        v0::Proposal {
            operation: v0::Operation::SetProxy {
                id: PENDING_PRICE_ID,
                proxy: Some(pending_proxy()),
            },
            created_at: PROPOSAL_4_CREATED_AT,
            ttl: Nanoseconds::from_secs(30),
            created_by: PROXY_ORACLE_ACCOUNT_ID.parse().unwrap(),
        },
    );
    state.governance.proposals.insert(
        5,
        v0::Proposal {
            operation: v0::Operation::SetActionTtl {
                new_ttl: Nanoseconds::from_secs(90),
            },
            created_at: PROPOSAL_5_CREATED_AT,
            ttl: Nanoseconds::from_secs(30),
            created_by: PROXY_ORACLE_ACCOUNT_ID.parse().unwrap(),
        },
    );
    state.governance.proposals.flush();

    near_sdk::env::state_write(&state);
    write_state_version(0);

    with_mocked_blockchain(|b| b.take_storage())
}

fn migration() -> state::migration::Migration {
    state::migration::Migration::from(state::migration::V0ToV1)
}

#[test]
#[ignore = "fixture generator"]
fn generate_v0_state_patch() {
    let state_patch = build_patch();
    fs::write(patch_path(), borsh::to_vec(&state_patch).unwrap()).unwrap();
}

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn init_writes_current_state_version() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let proxy = harness.deploy_proxy_oracle().await?;
    let network = &harness.network;

    assert_eq!(
        common::view::<u32>(network, &proxy, "get_target_state_version", json!({})).await?,
        1
    );
    assert_eq!(
        common::view::<u32>(network, &proxy, "get_stored_state_version", json!({})).await?,
        1
    );
    assert!(!common::view::<bool>(network, &proxy, "needs_migration", json!({})).await?);

    Ok(())
}

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migrate_v0_fixture_exactly() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let proxy = common::deploy_from_patch(&harness, patch()).await?;
    let network = &harness.network;

    assert_eq!(
        common::view::<u32>(network, &proxy, "get_stored_state_version", json!({})).await?,
        0
    );
    assert_eq!(
        common::view::<u32>(network, &proxy, "get_target_state_version", json!({})).await?,
        1
    );
    assert!(common::view::<bool>(network, &proxy, "needs_migration", json!({})).await?);

    common::call(network, &proxy, &proxy, "migrate", migration(), 300, 0).await?;

    assert_eq!(
        common::view::<u32>(network, &proxy, "get_stored_state_version", json!({})).await?,
        1
    );
    assert_eq!(
        common::view::<u32>(network, &proxy, "get_target_state_version", json!({})).await?,
        1
    );
    assert!(!common::view::<bool>(network, &proxy, "needs_migration", json!({})).await?);

    let mut proxies: Vec<PriceIdentifier> = common::view(
        network,
        &proxy,
        "list_proxies",
        json!({ "offset": null, "count": null }),
    )
    .await?;
    proxies.sort();
    assert_eq!(proxies, vec![BTC_PRICE_ID, ETH_PRICE_ID, STNEAR_PRICE_ID]);

    assert_eq!(
        common::view::<Option<Proxy<Source>>>(
            network,
            &proxy,
            "get_proxy",
            json!({ "id": BTC_PRICE_ID }),
        )
        .await?
        .unwrap(),
        expected_btc_proxy()
    );
    assert_eq!(
        common::view::<Option<Proxy<Source>>>(
            network,
            &proxy,
            "get_proxy",
            json!({ "id": ETH_PRICE_ID }),
        )
        .await?
        .unwrap(),
        expected_eth_proxy()
    );
    assert_eq!(
        common::view::<Option<Proxy<Source>>>(
            network,
            &proxy,
            "get_proxy",
            json!({ "id": STNEAR_PRICE_ID }),
        )
        .await?
        .unwrap(),
        expected_stnear_proxy()
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migrate_is_private() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let proxy = common::deploy_from_patch(&harness, patch()).await?;

    let caller: AccountId = "caller.near".parse()?;
    common::create_account(&harness.sandbox, &caller, NearToken::from_near(10)).await?;

    let result = common::try_call(
        &harness.network,
        &proxy,
        &caller,
        "migrate",
        migration(),
        300,
        0,
    )
    .await?;
    common::assert_failure_contains(result, "migrate function is private");

    Ok(())
}
