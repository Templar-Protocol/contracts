#![allow(clippy::unwrap_used)]

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use near_api::{types::AccountId, Contract, NetworkConfig};
use templar_common::{oracle::pyth::PriceIdentifier, Nanoseconds};
use templar_gateway_methods_spec::proxy_oracle;
use templar_gateway_testing::{ManagedAccountId, SandboxHarness};
use templar_gateway_types::OperationStatus;
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

/// A `from_version` the `Migration` enum cannot represent, so the contract has to
/// reject it at deserialization — which is the point of the revert test.
#[derive(near_sdk::serde::Serialize)]
#[serde(crate = "near_sdk::serde")]
struct InvalidMigration {
    from_version: &'static str,
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

/// Re-dump the fixture from the live mainnet deployment. Raw `near_api` on
/// purpose: this reads an account's whole storage trie off mainnet rather than
/// calling a contract method, so it has no gateway operation to go through.
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
    let client = harness.client()?;
    let proxy = common::deploy_from_patch(&harness, patch()).await?;

    // `migrate` is `#[private]` and has no typed gateway operation of its own —
    // see `migrate_v0.rs::migrate_v0_fixture_exactly`.
    harness
        .call_function(
            &ManagedAccountId(proxy.clone()),
            &proxy,
            "migrate",
            migration(),
        )
        .await?;

    let version = harness.contract_state_version(&proxy).await?;
    assert_eq!(version.stored, 1);
    assert!(!version.needs_migration);

    let mut proxies = client
        .read(proxy_oracle::ListProxies {
            oracle_id: proxy.clone(),
            offset: None,
            count: None,
        })
        .await?
        .proxies;
    proxies.sort();
    assert_eq!(proxies, vec![USDC_PRICE_ID, USTRY_PRICE_ID]);

    for (price_id, expected) in [
        (USTRY_PRICE_ID, expected_ustry_proxy()),
        (USDC_PRICE_ID, expected_usdc_proxy()),
    ] {
        let stored = client
            .read(proxy_oracle::GetProxy {
                oracle_id: proxy.clone(),
                id: price_id,
            })
            .await?
            .proxy;
        assert_eq!(stored.unwrap(), expected);
    }

    Ok(())
}

#[tokio::test]
async fn failed_migration_reverts_contract_code() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let account_id = harness.proxy_oracle_signer_account_id.0.clone();

    // Reproduce the legacy v0 contract (v0 code + v0 state), without migrating.
    harness
        .deploy_code(
            &account_id,
            templar_gateway_testing::wasm::released(
                templar_gateway_testing::ArtifactId::ProxyOracle,
                "0.1.0",
            )
            .await,
        )
        .await?;
    harness.patch_state(&account_id, patch()).await?;

    // Atomically deploy the current wasm and migrate with an invalid version in
    // one transaction. The migrate call fails, so the whole transaction — the
    // code deploy included — must revert, leaving the contract on the v0 code.
    let result = harness
        .try_deploy_and_init(
            &account_id,
            templar_gateway_testing::wasm::proxy_oracle().await.to_vec(),
            "migrate",
            InvalidMigration {
                from_version: "invalid",
            },
        )
        .await?;
    assert_eq!(
        result.operation.status,
        OperationStatus::Failed,
        "invalid migration should fail"
    );

    // The deploy reverted with the migrate, so the contract still reports v0.
    assert_eq!(harness.contract_version(&account_id).await?, "0.1.0");

    Ok(())
}
