#![allow(clippy::unwrap_used)]

use std::fs;

use near_workspaces::{network::Sandbox, Worker};
use templar_common::time::Nanoseconds;
use templar_proxy_oracle_kernel::state::legacy::v0;
use test_utils::{worker, ProxyOracleController};

#[path = "support/migration_fixture.rs"]
mod fixture;

#[rstest::rstest]
#[tokio::test]
#[ignore = "fixture generator"]
async fn generate_v0_state_patch(#[future(awt)] worker: Worker<Sandbox>) {
    let contract = worker
        .dev_deploy(ProxyOracleController::wasm_v0())
        .await
        .unwrap();
    contract
        .call("new")
        .args_json(near_sdk::serde_json::json!({}))
        .transact()
        .await
        .unwrap()
        .unwrap();

    let accounts = fixture::create_fixture_accounts(&worker).await;

    fixture::gov_create(
        &contract,
        0,
        &v0::Operation::SetProxy {
            id: fixture::BTC_PRICE_ID,
            proxy: Some(fixture::executed_btc_proxy(&accounts)),
        },
    )
    .await;
    fixture::gov_execute(&contract, 0).await;

    fixture::gov_create(
        &contract,
        1,
        &v0::Operation::SetProxy {
            id: fixture::ETH_PRICE_ID,
            proxy: Some(fixture::executed_eth_proxy(&accounts)),
        },
    )
    .await;
    fixture::gov_execute(&contract, 1).await;

    fixture::gov_create(
        &contract,
        2,
        &v0::Operation::SetProxy {
            id: fixture::STNEAR_PRICE_ID,
            proxy: Some(fixture::executed_stnear_proxy(&accounts)),
        },
    )
    .await;
    fixture::gov_execute(&contract, 2).await;

    fixture::gov_create(
        &contract,
        3,
        &v0::Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(30),
        },
    )
    .await;
    fixture::gov_execute(&contract, 3).await;

    fixture::gov_create(
        &contract,
        4,
        &v0::Operation::SetProxy {
            id: fixture::PENDING_PRICE_ID,
            proxy: Some(fixture::pending_proxy(&accounts)),
        },
    )
    .await;
    fixture::gov_create(
        &contract,
        5,
        &v0::Operation::SetActionTtl {
            new_ttl: Nanoseconds::from_secs(90),
        },
    )
    .await;

    let state_patch = contract
        .view_state()
        .await
        .unwrap()
        .into_iter()
        .collect::<fixture::StatePatch>();
    fs::write(
        fixture::patch_path(),
        near_sdk::borsh::to_vec(&state_patch).unwrap(),
    )
    .unwrap();
}
