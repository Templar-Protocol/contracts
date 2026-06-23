//! Ported from `contract/market/tests/maximum_borrow_duration_ms.rs`.
//!
//! The borrow-status expiration *logic* is covered by a pure unit test in
//! `templar-common` (`borrow_status_liquidation_on_expiration`); this confirms
//! the on-chain view reflects it. The original slept 2s of wall-clock; here we
//! advance time deterministically with `fast_forward`.

use anyhow::{Context, Result};
use near_sdk::json_types::U64;
use rstest::rstest;
use templar_common::borrow::{BorrowStatus, LiquidationReason};
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn liquidatable_after_expiration(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_maximum_duration_ms = Some(U64(1000));
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;
    harness.borrow(&borrow_user, &market, 100).await?;

    // Well-collateralized and within its duration → healthy.
    let prices = harness.get_oracle_prices(&market).await?;
    let status = harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?;
    assert!(
        status.is_healthy(),
        "should be healthy before expiration: {status:?}"
    );

    // Advance past the maximum borrow duration.
    harness.fast_forward(200).await?;

    let prices = harness.get_oracle_prices(&market).await?;
    let status = harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?;
    assert_eq!(
        status,
        BorrowStatus::Liquidation(LiquidationReason::Expiration),
        "should be liquidatable by expiration after the duration elapses",
    );

    Ok(())
}
