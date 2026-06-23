//! Ported from `contract/market/tests/accumulations.rs`: anyone (not just the
//! account owner) can drive interest accrual and yield harvesting. A smoke test
//! that each permissionless call succeeds.

use anyhow::Result;
use rstest::rstest;
use templar_common::market::HarvestYieldMode;
use templar_gateway_testing::{harness, SandboxHarness};

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn third_party_accumulation_executor(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let third_party = harness.create_user("third").await?;
    for user in [&supply_user, &borrow_user, &third_party] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;
    harness.borrow(&borrow_user, &market, 1000).await?;

    // A third party (and the owner) can apply interest to the borrow position.
    harness
        .apply_interest(&third_party, &market, Some(borrow_user.0.clone()), None)
        .await?;
    harness
        .apply_interest(&borrow_user, &market, Some(borrow_user.0.clone()), None)
        .await?;

    harness.repay(&borrow_user, &market, 1100, None).await?;

    // A third party can harvest yield on behalf of the supplier, in either mode.
    harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    harness
        .harvest_yield(&third_party, &market, Some(supply_user.0.clone()))
        .await?;
    harness
        .harvest_yield_with_mode(
            &supply_user,
            &market,
            Some(supply_user.0.clone()),
            Some(HarvestYieldMode::SnapshotLimit(100)),
        )
        .await?;
    harness
        .harvest_yield_with_mode(
            &third_party,
            &market,
            Some(supply_user.0.clone()),
            Some(HarvestYieldMode::SnapshotLimit(100)),
        )
        .await?;

    Ok(())
}
