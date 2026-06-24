//! Ported from `contract/market/tests/0_many_snapshots.rs`: as snapshots
//! accumulate, the per-borrow gas must not grow with the snapshot count. Gas is
//! read via the harness `operation_gas_burnt` helper. Long-running (256 borrows).

use anyhow::Result;
use rstest::rstest;
use templar_common::time_chunk::TimeChunkConfiguration;
use templar_gateway_testing::{harness, SandboxHarness};

#[allow(clippy::cast_precision_loss)]
fn linear_regression_slope(data: &[(f64, f64)]) -> f64 {
    let n = data.len() as f64;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_xx = 0.0;
    for &(x, y) in data {
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_xx += x * x;
    }
    (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x)
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn many_snapshots(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 100_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 200_000)
        .await?;

    // 256 = 2 * the snapshot-container chunk size (128), so we cross a boundary.
    let mut gas_record = Vec::with_capacity(256);
    for i in 0..256u32 {
        let result = harness.borrow(&borrow_user, &market, 100).await?;
        let gas = harness.operation_gas_burnt(&result).await?;
        #[allow(clippy::cast_precision_loss)]
        gas_record.push((f64::from(i), gas as f64));
    }

    let slope = linear_regression_slope(&gas_record);
    assert!(slope < 1e10, "gas growing with snapshots (slope {slope})");

    Ok(())
}
