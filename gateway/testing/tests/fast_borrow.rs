//! WS3 spike: `contract/market/tests/fast_borrow.rs` ported onto the in-process
//! gateway harness. Locks the `SandboxHarness` ops API before batch porting.
//!
//! Node-backed, so gated behind `#[ignore]`: run with
//! `cargo nextest run -p templar-gateway-testing --run-ignored all`.

use anyhow::{Context, Result};
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use templar_gateway_testing::SandboxHarness;
use test_utils::to_price;

#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fast_borrow_is_not_free() -> Result<()> {
    let harness = SandboxHarness::start().await?;

    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(60 * 1000);
            // 1 minute
        })
        .await?;

    // Both assets priced at 1.0 so collateral covers the borrow.
    let oracle = market.configuration.price_oracle_configuration.clone();
    harness
        .set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.borrow_asset_price_id,
            Some(to_price(1.0)),
        )
        .await?;
    harness
        .set_mock_oracle_pyth_price(
            oracle.account_id.clone(),
            oracle.collateral_asset_price_id,
            Some(to_price(1.0)),
        )
        .await?;

    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_000_000)
        .await?;

    let snapshot_len_before = harness.get_finalized_snapshots_len(&market).await?;
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000_000).await?;

    // Repay the exact amount borrowed; interest accrued over the borrow means
    // some liability must remain.
    harness
        .repay(&borrow_user, &market, 1_000_000, None)
        .await?;

    let borrow_position = harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .context("borrow position missing")?;

    assert!(
        !borrow_position.get_total_borrow_asset_liability().is_zero(),
        "borrow position should not have zero liability",
    );

    let snapshot_len_after = harness.get_finalized_snapshots_len(&market).await?;
    assert_eq!(
        snapshot_len_before, snapshot_len_after,
        "test should run within a single snapshot",
    );

    Ok(())
}
