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

use anyhow::Result;
use near_api::types::AccountId;
use near_sdk::{json_types::U128, serde_json::json, AccountIdRef, Gas};
use templar_common::oracle::pyth::{self, OracleResponse, PriceIdentifier, PythTimestamp};
use templar_gateway_methods_spec::lst_oracle;
use templar_gateway_testing::{ManagedAccountId, SandboxHarness};
use templar_proxy_oracle_near_common::price_transformer::{Call, PriceTransformer};
use test_utils::{DEFAULT_BORROW_PRICE_ID, DEFAULT_COLLATERAL_PRICE_ID};

const COLLATERAL_LST_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cc11000000000000000000000000000000000000000000000000000000000000"
));

/// A Pyth price at the current *chain* time, not the host clock — see
/// [`SandboxHarness::chain_timestamp`].
#[allow(clippy::cast_possible_wrap)]
async fn pyth_price_now(harness: &SandboxHarness, value: i64) -> Result<pyth::Price> {
    let now = harness.chain_timestamp().await?.as_secs() as i64;
    Ok(pyth::Price {
        price: value.into(),
        conf: 0.into(),
        expo: 0,
        publish_time: PythTimestamp::from_secs(now),
    })
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
        near_sdk::serde_json::Value::Null,
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
async fn lst_oracle() -> Result<()> {
    let harness = SandboxHarness::start().await?;
    let client = harness.client()?;

    // Reuse the harness's mock FT as the LST collateral asset, exposing a 2:1
    // redemption rate (2 * 10^24, i.e. 24-decimal native). Mock-only method, so
    // it goes through the generic function-call escape hatch.
    let collateral_asset = harness.ft_contract_id.clone();
    harness
        .call_function(
            &ManagedAccountId(collateral_asset.clone()),
            &collateral_asset,
            "set_redemption_rate",
            json!({ "redemption_rate": U128(2 * 10u128.pow(24)) }),
        )
        .await?;

    // Underlying (mock pyth) oracle with the base borrow/collateral feeds.
    let underlying = harness.deploy_mock_oracle("oracle").await?;
    harness
        .set_mock_oracle_pyth_price(
            underlying.clone(),
            DEFAULT_COLLATERAL_PRICE_ID,
            Some(pyth_price_now(&harness, 100_000).await?),
        )
        .await?;
    harness
        .set_mock_oracle_pyth_price(
            underlying.clone(),
            DEFAULT_BORROW_PRICE_ID,
            Some(pyth_price_now(&harness, 100_000).await?),
        )
        .await?;

    // LST oracle wrapping the underlying oracle, with a transformer for the LST
    // collateral feed.
    let lst_oracle_id = harness
        .deploy_lst_oracle("lst-oracle", underlying.clone())
        .await?;
    harness
        .create_lst_transformer(
            lst_oracle_id.clone(),
            COLLATERAL_LST_ID,
            expected_transformer(&collateral_asset),
        )
        .await?;

    // The LST oracle reports its backing oracle.
    let underlying_oracle_actual: AccountId = client
        .read(lst_oracle::GetOracleId {
            oracle_id: lst_oracle_id.clone(),
        })
        .await?
        .pyth_oracle_id;
    assert_eq!(underlying_oracle_actual, underlying);

    // The transformer is listed and round-trips.
    let transformers = client
        .read(lst_oracle::ListTransformers {
            oracle_id: lst_oracle_id.clone(),
            pagination: templar_gateway_types::common::Pagination::default(),
        })
        .await?
        .price_ids;
    assert_eq!(transformers, vec![COLLATERAL_LST_ID]);

    let transformer = client
        .read(lst_oracle::GetTransformer {
            oracle_id: lst_oracle_id.clone(),
            price_identifier: COLLATERAL_LST_ID,
        })
        .await?
        .transformer;
    assert_eq!(
        transformer.unwrap(),
        expected_transformer(&collateral_asset)
    );

    // `price_feed_exists` and `list_ema_prices_no_older_than` return a
    // `PromiseOrValue` here — they fan out to the underlying oracle — so unlike
    // the proxy oracle's plain views they cannot be served as view calls. They
    // are driven as function calls and read back from the return value.
    let lst_signer = ManagedAccountId(lst_oracle_id.clone());

    // The transformer feed plus both forwarded underlying feeds exist; an
    // unknown feed does not.
    for should_exist in [
        COLLATERAL_LST_ID,
        DEFAULT_COLLATERAL_PRICE_ID,
        DEFAULT_BORROW_PRICE_ID,
    ] {
        let exists: bool = harness
            .call_function_json(
                &lst_signer,
                &lst_oracle_id,
                "price_feed_exists",
                json!({ "price_identifier": should_exist }),
            )
            .await?;
        assert!(exists, "price ID {should_exist} should exist");
    }
    let missing: bool = harness
        .call_function_json(
            &lst_signer,
            &lst_oracle_id,
            "price_feed_exists",
            json!({ "price_identifier": PriceIdentifier([0x88; 32]) }),
        )
        .await?;
    assert!(!missing);

    // End-to-end price resolution: the borrow feed passes through unchanged, and
    // the LST collateral feed is the underlying collateral price scaled by the
    // 2:1 redemption rate.
    let oracle_response: OracleResponse = harness
        .call_function_json(
            &lst_signer,
            &lst_oracle_id,
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

    Ok(())
}
