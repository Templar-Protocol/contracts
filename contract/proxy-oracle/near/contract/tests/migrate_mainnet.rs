#![allow(clippy::unwrap_used)]

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use near_workspaces::{
    network::Sandbox, operations::Function, types::Gas, AccountId, Contract, Worker,
};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest, state};
use test_utils::{
    assert_all_outcomes_success, controller::migration::MigrationController,
    pyth_price_id::stable::CRYPTO_USDC_USD, worker, ProxyOracleController,
};

type StatePatch = HashMap<Vec<u8>, Vec<u8>>;

const USTRY_PRICE_ID: PriceIdentifier =
    PriceIdentifier(*b"USTRY\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
const USDC_PRICE_ID: PriceIdentifier =
    PriceIdentifier(*b"USDC\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");

fn patch_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.borsh")
}

fn patch() -> StatePatch {
    near_sdk::borsh::from_slice(include_bytes!(
        "./migration/mainnet_proxy_oracle_ixlmustry_ixlmusdc.borsh"
    ))
    .unwrap()
}

async fn deploy_legacy_from_patch(worker: &Worker<Sandbox>, state_patch: StatePatch) -> Contract {
    let contract = worker
        .dev_deploy(ProxyOracleController::wasm_v0())
        .await
        .unwrap();

    for (key, value) in state_patch {
        worker
            .patch_state(contract.id(), &key, &value)
            .await
            .unwrap();
    }

    contract
}

async fn deploy_from_patch(
    worker: &Worker<Sandbox>,
    state_patch: StatePatch,
) -> ProxyOracleController {
    let contract = deploy_legacy_from_patch(worker, state_patch).await;
    let wasm = ProxyOracleController::wasm().await;
    let result = contract
        .as_account()
        .batch(contract.id())
        .deploy(wasm)
        .call(
            Function::new("migrate")
                .args_json(state::migration::Migration::from(state::migration::V0ToV1))
                .gas(Gas::from_tgas(250)),
        )
        .transact()
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert_all_outcomes_success(&result);

    ProxyOracleController { contract }
}
fn expected_ustry_proxy() -> Proxy<Source> {
    Proxy::median_low(
        [
            OracleRequest::redstone("redstone-adapter.v1.tmplr.near".parse().unwrap(), "USTRY")
                .into(),
        ],
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(60)),
            Some(Nanoseconds::from_secs(10)),
        ),
    )
}

fn expected_usdc_proxy() -> Proxy<Source> {
    Proxy::median_low(
        [
            OracleRequest::redstone("redstone-adapter.v1.tmplr.near".parse().unwrap(), "USDC")
                .into(),
            OracleRequest::pyth("pyth-oracle.near".parse().unwrap(), CRYPTO_USDC_USD).into(),
        ],
        FreshnessFilter::new(
            Some(Nanoseconds::from_secs(60)),
            Some(Nanoseconds::from_secs(10)),
        ),
    )
}

#[tokio::test]
#[ignore = "fixture generator"]
async fn generate_mainnet_state_patch() {
    let worker = near_workspaces::mainnet().await.unwrap();
    let account_id: AccountId = "proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near"
        .parse()
        .unwrap();
    let state_patch = worker
        .view_state(&account_id)
        .await
        .unwrap()
        .into_iter()
        .collect::<StatePatch>();

    fs::write(patch_path(), near_sdk::borsh::to_vec(&state_patch).unwrap()).unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn migrate_mainnet_patch_exactly(#[future(awt)] worker: Worker<Sandbox>) {
    let proxy = deploy_from_patch(&worker, patch()).await;

    assert_eq!(proxy.get_stored_state_version().await, 1);
    assert!(!proxy.needs_migration().await);

    let mut proxies = proxy.list_proxies(None, None).await;
    proxies.sort();
    assert_eq!(proxies, vec![USDC_PRICE_ID, USTRY_PRICE_ID]);

    assert_eq!(
        proxy.get_proxy(USTRY_PRICE_ID).await.unwrap(),
        expected_ustry_proxy()
    );
    assert_eq!(
        proxy.get_proxy(USDC_PRICE_ID).await.unwrap(),
        expected_usdc_proxy()
    );
}

#[rstest::rstest]
#[tokio::test]
async fn failed_migration_reverts_contract_code(#[future(awt)] worker: Worker<Sandbox>) {
    let contract = deploy_legacy_from_patch(&worker, patch()).await;
    let result = contract
        .as_account()
        .batch(contract.id())
        .deploy(ProxyOracleController::wasm().await)
        .call(
            Function::new("migrate")
                .args_json(near_sdk::serde_json::json!({ "from_version": "invalid" }))
                .gas(Gas::from_tgas(250)),
        )
        .transact()
        .await
        .unwrap();
    assert!(result.is_failure(), "invalid migration should fail");

    let metadata: near_sdk::serde_json::Value = contract
        .view("contract_source_metadata")
        .args_json(())
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(metadata["version"], "0.1.0");
}
