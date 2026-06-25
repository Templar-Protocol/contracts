#![allow(clippy::unwrap_used)]

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use near_api::{types::AccountId, Contract, NetworkConfig};
use serde_json::json;
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_gateway_testing::SandboxHarness;
use templar_proxy_oracle_kernel::proxy::{FreshnessFilter, Proxy};
use templar_proxy_oracle_near_common::{input::Source, request::OracleRequest, state};
use test_utils::pyth_price_id::stable::CRYPTO_USDC_USD;

use common::StatePatch;

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

fn migration() -> state::migration::Migration {
    state::migration::Migration::from(state::migration::V0ToV1)
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
async fn generate_mainnet_state_patch() -> Result<()> {
    let network = NetworkConfig::mainnet();
    let account_id: AccountId = "proxy-oracle-ixlmustry-ixlmusdc.v1.tmplr.near".parse()?;
    let storage = Contract(account_id)
        .view_storage()
        .fetch_from(&network)
        .await?
        .data;
    let state_patch: StatePatch = storage
        .values
        .into_iter()
        .map(|entry| {
            (
                STANDARD.decode(entry.key.0).unwrap(),
                STANDARD.decode(entry.value.0).unwrap(),
            )
        })
        .collect();
    fs::write(patch_path(), near_sdk::borsh::to_vec(&state_patch).unwrap()).unwrap();
    Ok(())
}

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn migrate_mainnet_patch_exactly() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let proxy = common::deploy_from_patch(&harness, patch()).await?;
    let network = &harness.network;

    common::call(network, &proxy, &proxy, "migrate", migration(), 300, 0).await?;

    assert_eq!(
        common::view::<u32>(network, &proxy, "get_stored_state_version", json!({})).await?,
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
    assert_eq!(proxies, vec![USDC_PRICE_ID, USTRY_PRICE_ID]);

    assert_eq!(
        common::view::<Option<Proxy<Source>>>(
            network,
            &proxy,
            "get_proxy",
            json!({ "id": USTRY_PRICE_ID }),
        )
        .await?
        .unwrap(),
        expected_ustry_proxy()
    );
    assert_eq!(
        common::view::<Option<Proxy<Source>>>(
            network,
            &proxy,
            "get_proxy",
            json!({ "id": USDC_PRICE_ID }),
        )
        .await?
        .unwrap(),
        expected_usdc_proxy()
    );

    Ok(())
}
