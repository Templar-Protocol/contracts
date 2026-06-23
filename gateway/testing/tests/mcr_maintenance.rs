//! Ported from `contract/market/tests/mcr_maintenance.rs`.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{dec, fee::Fee, Decimal};
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

const HEALTHY_AFTER_BORROW: &str = "Borrow position must be healthy after borrow";
const HEALTHY_AFTER_WITHDRAWAL: &str =
    "Borrow position must be healthy after collateral withdrawal";

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.000000000000000000000000000001"), dec!("1.000000000000000000000000000001"))]
#[case(dec!("1.00000001"), dec!("1.1"))]
#[case(dec!("1.00000000000000000000000000000000001"), dec!("5"))]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn success_above_mcr_maintenance(
    #[future(awt)] harness: SandboxHarness,
    #[case] liquidation: Decimal,
    #[case] maintenance: Decimal,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
            c.liquidation_maximum_spread = Decimal::ZERO;
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    let collateral_amount = ((1000u32
        * (1u32 + market.configuration.single_snapshot_maximum_interest()))
    .to_u128_ceil()
    .unwrap()
        * maintenance
        + 1u32)
        .to_u128_ceil()
        .unwrap();

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, collateral_amount)
        .await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    harness.borrow(&borrow_user, &market, 1000).await?;
    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;

    assert_eq!(balance_before + 1000, balance_after);
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_borrow_asset_principal()
        ),
        1000,
    );

    Ok(())
}

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.001"), dec!("1.001"))]
#[case(dec!("1.001"), dec!("1.1"))]
#[case(dec!("1.001"), dec!("2"))]
#[case(dec!("1.001"), dec!("5"))]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fail_below_mcr_maintenance(
    #[future(awt)] harness: SandboxHarness,
    #[case] liquidation: Decimal,
    #[case] maintenance: Decimal,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
            c.liquidation_maximum_spread = Decimal::ZERO;
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .collateralize(
            &borrow_user,
            &market,
            (1000u32 * maintenance).to_u128_floor().unwrap() - 1,
        )
        .await?;

    let result = harness.try_borrow(&borrow_user, &market, 1000).await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains(HEALTHY_AFTER_BORROW),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    Ok(())
}

#[rstest]
#[case(dec!("1.2"), dec!("1.4"))]
#[case(dec!("1.001"), dec!("1.1"))]
#[case(dec!("1.5"), dec!("2"))]
#[case(dec!("1.5"), dec!("5"))]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn not_in_liquidation_if_below_mcr_maintenance(
    #[future(awt)] harness: SandboxHarness,
    #[case] liquidation: Decimal,
    #[case] maintenance: Decimal,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = liquidation;
            c.borrow_mcr_maintenance = maintenance;
            c.liquidation_maximum_spread = Decimal::ZERO;
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    let collateral_amount = ((1000u32
        * (1u32 + market.configuration.single_snapshot_maximum_interest()))
    .to_u128_ceil()
    .unwrap()
        * maintenance)
        .to_u128_ceil()
        .unwrap();

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, collateral_amount)
        .await?;
    harness.borrow(&borrow_user, &market, 1000).await?;

    // Just below the maintenance ratio, but still above the liquidation ratio.
    harness.set_asset_prices(&market, 1.0, 0.99).await?;

    let prices = harness.get_oracle_prices(&market).await?;
    let status = harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?;
    assert!(
        !status.is_liquidation(),
        "below maintenance but above liquidation must not be liquidatable: {status:?}",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn withdraw_collateral_below_mcr_maintenance(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_mcr_liquidation = dec!("1.2");
            c.borrow_mcr_maintenance = dec!("1.5");
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    let collateral_amount = ((1000u32
        * (1u32 + market.configuration.single_snapshot_maximum_interest()))
    .to_u128_ceil()
    .unwrap()
        * dec!("1.5"))
    .to_u128_ceil()
    .unwrap();

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, collateral_amount)
        .await?;
    harness.borrow(&borrow_user, &market, 1000).await?;

    // Withdrawing any collateral drops the position below maintenance.
    let result = harness
        .try_withdraw_collateral(&borrow_user, &market, 1)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains(HEALTHY_AFTER_WITHDRAWAL),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    Ok(())
}
