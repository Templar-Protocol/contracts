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

#[tokio::test]
async fn failed_migration_reverts_contract_code() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = &harness.network;
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    // Reproduce the legacy v0 contract (v0 code + v0 state), without migrating.
    common::deploy_code(
        network,
        &account_id,
        templar_gateway_testing::wasm::released(
            templar_gateway_testing::ArtifactId::ProxyOracle,
            "0.1.0",
        )
        .to_vec(),
    )
    .await?;
    harness.patch_state(&account_id, patch()).await?;

    // Atomically deploy the current wasm and migrate with an invalid version in
    // one transaction. The migrate call fails, so the whole transaction — the
    // code deploy included — must revert, leaving the contract on the v0 code.
    let result = Contract::deploy(account_id.clone())
        .use_code(templar_gateway_testing::wasm::proxy_oracle().await.to_vec())
        .with_init_call("migrate", json!({ "from_version": "invalid" }))?
        .max_gas()
        .with_signer(common::signer())
        .wait_until(templar_gateway_testing::TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?;
    assert!(result.is_failure(), "invalid migration should fail");

    // The deploy reverted with the migrate, so the contract still reports v0.
    let metadata: serde_json::Value =
        common::view(network, &account_id, "contract_source_metadata", json!({})).await?;
    assert_eq!(metadata["version"], "0.1.0");

    Ok(())
}
