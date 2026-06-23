//! Ported from `contract/market/tests/maximum_borrow_asset_usage_ratio.rs`.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::Decimal;
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn borrow_within_maximum_usage_ratio(
    #[future(awt)] harness: SandboxHarness,
    #[values(1, 50, 99, 100)] percent: u16,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
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

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    let amount = u128::from(percent) * 10 - 1;
    harness.borrow(&borrow_user, &market, amount).await?;
    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;

    assert_eq!(balance_before + amount, balance_after);
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_borrow_asset_principal()
        ),
        amount,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn borrow_exceeds_maximum_usage_ratio(
    #[future(awt)] harness: SandboxHarness,
    #[values(1, 50, 99, 100)] percent: u16,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_asset_maximum_usage_ratio = Decimal::from(percent) / 100u32;
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

    let result = harness
        .try_borrow(&borrow_user, &market, u128::from(percent) * 10 + 1)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("Insufficient borrow asset available"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    Ok(())
}
