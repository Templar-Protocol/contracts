#![allow(clippy::unwrap_used)]

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use near_workspaces::{network::Sandbox, AccountId, Worker};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest, state};
use test_utils::{
    assert_all_outcomes_success, controller::migration::MigrationController,
    pyth_price_id::stable::CRYPTO_USDC_USD, worker, ContractController, GovernanceController,
    ProxyOracleController,
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

async fn deploy_from_patch(
    worker: &Worker<Sandbox>,
    state_patch: StatePatch,
) -> ProxyOracleController {
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

    let contract = contract
        .as_account()
        .deploy(ProxyOracleController::wasm().await)
        .await
        .unwrap()
        .unwrap();

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

    let result = proxy
        .migrate(
            proxy.contract().as_account(),
            state::migration::Migration::from(state::migration::V0ToV1),
        )
        .await;

    assert_all_outcomes_success(&result);
    assert_eq!(proxy.get_stored_state_version().await, 1);
    assert_eq!(proxy.gov_count().await, 0);

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
