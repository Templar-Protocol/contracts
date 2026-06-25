//! LST oracle integration test, driven over the gateway `SandboxHarness`.
//!
//! Covers the LST oracle's own behavior end-to-end: it wraps an underlying
//! (mock pyth) oracle, exposes a price transformer that normalizes a native LST
//! price by an on-chain redemption rate, and forwards non-transformer feeds to
//! the underlying oracle.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use std::sync::Arc;

use anyhow::{Context, Result};
use near_api::{types::AccountId, Contract, NetworkConfig, SecretKey, Signer};
use near_sdk::{
    json_types::U128,
    serde::{de::DeserializeOwned, Serialize},
    serde_json::{json, Value},
    AccountIdRef, Gas,
};
use near_token::NearToken;
use templar_common::oracle::pyth::{self, OracleResponse, PriceIdentifier, PythTimestamp};
use templar_gateway_testing::SandboxHarness;
use templar_proxy_oracle_near_common::price_transformer::{Call, PriceTransformer};
use test_utils::{DEFAULT_BORROW_PRICE_ID, DEFAULT_COLLATERAL_PRICE_ID};

const TEST_SECRET_KEY: &str =
    "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

const COLLATERAL_LST_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cc11000000000000000000000000000000000000000000000000000000000000"
));

fn signer() -> Result<Arc<Signer>> {
    let secret_key: SecretKey = TEST_SECRET_KEY.parse().context("parse test key")?;
    Signer::from_secret_key(secret_key).context("build test signer")
}

async fn view<T: DeserializeOwned + Send + Sync>(
    network: &NetworkConfig,
    contract_id: &AccountId,
    method: &str,
    args: impl Serialize,
) -> Result<T> {
    Ok(Contract(contract_id.clone())
        .call_function(method, args)
        .read_only::<T>()
        .fetch_from(network)
        .await?
        .data)
}

/// Signed call, asserting success and discarding the result.
async fn call(
    network: &NetworkConfig,
    contract_id: &AccountId,
    signer_id: &AccountId,
    method: &str,
    args: impl Serialize,
    deposit_yocto: u128,
) -> Result<()> {
    Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .gas(Gas::from_tgas(100))
        .deposit(NearToken::from_yoctonear(deposit_yocto))
        .with_signer(signer_id.clone(), signer()?)
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

/// Signed call returning the (possibly promise-resolved) JSON result.
async fn call_json<T: DeserializeOwned>(
    network: &NetworkConfig,
    contract_id: &AccountId,
    signer_id: &AccountId,
    method: &str,
    args: impl Serialize,
) -> Result<T> {
    let result = Contract(contract_id.clone())
        .call_function(method, args)
        .transaction()
        .gas(Gas::from_tgas(100))
        .with_signer(signer_id.clone(), signer()?)
        .send_to(network)
        .await?
        .into_result()?;
    Ok(result.json::<T>()?)
}

fn pyth_price_now(value: i64) -> pyth::Price {
    pyth::Price {
        price: value.into(),
        conf: 0.into(),
        expo: 0,
        publish_time: PythTimestamp::from_secs(
            std::time::UNIX_EPOCH
                .elapsed()
                .unwrap_or_default()
                .as_secs() as i64,
        ),
    }
}

fn norm_price(price: &pyth::Price) -> u64 {
    let p = u64::try_from(price.price.0).unwrap();
    let f = 10u64.pow(price.expo.unsigned_abs());
    if price.expo.is_negative() {
        p / f
    } else {
        p * f
    }
}

fn redemption_rate_call(account_id: &AccountIdRef) -> Call {
    Call::new(
        account_id,
        "redemption_rate",
        Value::Null,
        Gas::from_tgas(3),
    )
}

fn expected_transformer(collateral_asset: &AccountIdRef) -> PriceTransformer {
    PriceTransformer::lst(
        DEFAULT_COLLATERAL_PRICE_ID,
        24,
        redemption_rate_call(collateral_asset),
    )
}

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn lst_oracle() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let network = harness.network.clone();

    // Reuse the harness's mock FT as the LST collateral asset, exposing a 2:1
    // redemption rate (2 * 10^24, i.e. 24-decimal native).
    let collateral_asset = harness.ft_contract_id.clone();
    call(
        &network,
        &collateral_asset,
        &collateral_asset,
        "set_redemption_rate",
        json!({ "redemption_rate": U128(2 * 10u128.pow(24)) }),
        0,
    )
    .await?;

    // Underlying (mock pyth) oracle with the base borrow/collateral feeds.
    let underlying = harness.deploy_mock_oracle("oracle.near".parse()?).await?;
    harness
        .set_mock_oracle_pyth_price(
            underlying.clone(),
            DEFAULT_COLLATERAL_PRICE_ID,
            Some(pyth_price_now(100_000)),
        )
        .await?;
    harness
        .set_mock_oracle_pyth_price(
            underlying.clone(),
            DEFAULT_BORROW_PRICE_ID,
            Some(pyth_price_now(100_000)),
        )
        .await?;

    // LST oracle wrapping the underlying oracle, with a transformer for the LST
    // collateral feed.
    let lst_oracle = harness
        .deploy_lst_oracle("lst-oracle.near".parse()?, underlying.clone())
        .await?;
    harness
        .create_lst_transformer(
            lst_oracle.clone(),
            COLLATERAL_LST_ID,
            expected_transformer(&collateral_asset),
        )
        .await?;

    // The LST oracle reports its backing oracle.
    let underlying_oracle_actual: AccountId =
        view(&network, &lst_oracle, "oracle_id", json!({})).await?;
    assert_eq!(underlying_oracle_actual, underlying);

    // The transformer is listed and round-trips.
    let transformers: Vec<PriceIdentifier> = view(
        &network,
        &lst_oracle,
        "list_transformers",
        json!({ "offset": null, "count": null }),
    )
    .await?;
    assert_eq!(transformers, vec![COLLATERAL_LST_ID]);

    let transformer: Option<PriceTransformer> = view(
        &network,
        &lst_oracle,
        "get_transformer",
        json!({ "price_identifier": COLLATERAL_LST_ID }),
    )
    .await?;
    assert_eq!(
        transformer.unwrap(),
        expected_transformer(&collateral_asset)
    );

    // The transformer feed plus both forwarded underlying feeds exist; an
    // unknown feed does not.
    for should_exist in [
        COLLATERAL_LST_ID,
        DEFAULT_COLLATERAL_PRICE_ID,
        DEFAULT_BORROW_PRICE_ID,
    ] {
        let exists: bool = call_json(
            &network,
            &lst_oracle,
            &lst_oracle,
            "price_feed_exists",
            json!({ "price_identifier": should_exist }),
        )
        .await?;
        assert!(exists, "price ID {should_exist} should exist");
    }
    let missing: bool = call_json(
        &network,
        &lst_oracle,
        &lst_oracle,
        "price_feed_exists",
        json!({ "price_identifier": PriceIdentifier([0x88; 32]) }),
    )
    .await?;
    assert!(!missing);

    // End-to-end price resolution: the borrow feed passes through unchanged, and
    // the LST collateral feed is the underlying collateral price scaled by the
    // 2:1 redemption rate.
    let oracle_response: OracleResponse = call_json(
        &network,
        &lst_oracle,
        &lst_oracle,
        "list_ema_prices_no_older_than",
        json!({ "price_ids": [DEFAULT_BORROW_PRICE_ID, COLLATERAL_LST_ID], "age": 60 }),
    )
    .await?;

    assert_eq!(
        oracle_response
            .get(&DEFAULT_BORROW_PRICE_ID)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(100_000),
    );
    assert_eq!(
        oracle_response
            .get(&COLLATERAL_LST_ID)
            .unwrap()
            .as_ref()
            .map(norm_price),
        Some(200_000),
    );

    // TODO(ENG-388 follow-up): the original near-workspaces test also drove a
    // full market (supply -> activation -> collateralize -> borrow) against this
    // LST oracle and asserted the borrow position stayed healthy. That market
    // flow belongs to the market-domain `SandboxHarness` helpers (migrated
    // separately); re-add the borrow-health assertion once those land.

    Ok(())
}
