//! Ported from `contract/market/tests/liquidation.rs` (core liquidation flows).
//!
//! Rejected liquidations pay via `ft_transfer_call`, so the contract's rejection
//! is refunded and asserted here as "no effect" (balances + position unchanged).
//! `liquidatable_collateral_fmv`/`_with_spread` are harness helpers mirroring the
//! retired controller. The interest-only case advances time with `fast_forward`
//! instead of sleeping. The yield-distribution (`good_debt_under_mcr`), partial,
//! and extreme-price cases are ported separately.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{dec, Decimal};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};

/// Supply liquidity and post 500 collateral against a 300 borrow (the common
/// setup for the underwater cases).
async fn setup_underwater(
    harness: &SandboxHarness,
    market: &DeployedMarket,
) -> Result<(
    templar_gateway_types::ManagedAccountId,
    templar_gateway_types::ManagedAccountId,
)> {
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, market).await?;
    }
    harness
        .supply_and_harvest_until_activation(&supply_user, market, 1000)
        .await?;
    harness.collateralize(&borrow_user, market, 500).await?;
    harness.borrow(&borrow_user, market, 300).await?;
    Ok((borrow_user, liquidator))
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn successful_liquidation_totally_underwater(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let (borrow_user, liquidator) = setup_underwater(&harness, &market).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    // Collateral value 500 -> 250, so 250/300 ~ 83%: bad debt, all liquidatable.
    harness.set_asset_prices(&market, 1.0, 0.5).await?;
    let (collateral, price) = harness
        .liquidatable_collateral_fmv(&market, &borrow_user.0)
        .await?;
    assert_eq!(
        u128::from(collateral),
        500,
        "all collateral should be liquidatable"
    );

    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(price),
            Some(u128::from(collateral)),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before + u128::from(collateral),
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before - u128::from(price),
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn successful_liquidation_exactly_to_zero(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let (borrow_user, liquidator) = setup_underwater(&harness, &market).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    // Price chosen to liquidate 100% of both collateral and liability.
    harness.set_asset_prices(&market, 1.0, 2.0 / 3.0).await?;
    let (collateral, price) = harness
        .liquidatable_collateral_fmv(&market, &borrow_user.0)
        .await?;
    assert_eq!(u128::from(collateral), 500);

    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(price),
            Some(u128::from(collateral)),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before + u128::from(collateral),
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before - u128::from(price),
    );
    // Liquidating both liability and collateral to zero cleans up the position.
    assert!(harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .is_none());

    Ok(())
}

#[rstest]
#[case(120, 5, 0)]
#[case(120, 5, 2)]
#[case(120, 5, 5)]
#[case(110, 2, 1)]
#[case(150, 33, 32)]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn successful_liquidation_with_spread(
    #[future(awt)] harness: SandboxHarness,
    #[case] mcr: u16,
    #[case] maximum_spread_pct: u16,
    #[case] spread_pct: u16,
) -> Result<()> {
    assert!(spread_pct <= maximum_spread_pct);
    let maximum_spread = Decimal::from(maximum_spread_pct) / 100u32;
    let target_spread = Decimal::from(spread_pct) / 100u32;
    let mcr_dec = Decimal::from(mcr) / 100u32;

    let market = harness
        .deploy_full_market_with(move |c| {
            c.borrow_mcr_liquidation = mcr_dec;
            c.borrow_mcr_maintenance = mcr_dec;
            c.liquidation_maximum_spread = maximum_spread;
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, &market).await?;
    }
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;
    harness.borrow(&borrow_user, &market, 1000).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    // 2:1 collateralization, a bit under MCR.
    let collateral_price = (mcr_dec / 201u32 * 100u32).to_f64_lossy();
    harness
        .set_asset_prices(&market, 1.0, collateral_price)
        .await?;

    let (collateral, fmv_price) = harness
        .liquidatable_collateral_fmv(&market, &borrow_user.0)
        .await?;
    let price = (u128::from(fmv_price) * (Decimal::ONE - target_spread))
        .to_u128_ceil()
        .context("spread conversion overflow")?;

    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            price,
            Some(u128::from(collateral)),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before + u128::from(collateral),
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before - price,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fail_liquidation_too_little_attached(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let (borrow_user, liquidator) = setup_underwater(&harness, &market).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    harness.set_asset_prices(&market, 1.0, 0.5).await?;
    // 500 collateral demanded but only 150 attached: rejected and refunded.
    harness
        .try_liquidate(&liquidator, &market, &borrow_user.0, 150, Some(500))
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before,
        "no collateral from a rejected liquidation",
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before,
        "rejected liquidation is refunded",
    );

    let position = harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .context("borrow position missing")?;
    assert_eq!(u128::from(position.get_borrow_asset_principal()), 300);
    assert_eq!(u128::from(position.collateral_asset_deposit), 500);

    let prices = harness.get_oracle_prices(&market).await?;
    assert!(harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?
        .is_liquidation());

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn fail_liquidation_healthy_borrow(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let (borrow_user, liquidator) = setup_underwater(&harness, &market).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    // Price unchanged: the position is healthy, so liquidation is rejected.
    harness
        .try_liquidate(&liquidator, &market, &borrow_user.0, 300, Some(500))
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before,
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before,
    );

    let position = harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .context("borrow position missing")?;
    assert_eq!(u128::from(position.get_borrow_asset_principal()), 300);
    assert_eq!(u128::from(position.collateral_asset_deposit), 500);

    let prices = harness.get_oracle_prices(&market).await?;
    assert!(harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?
        .is_healthy());

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn successful_liquidation_only_from_interest(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_mcr_liquidation = dec!("1.9997");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = templar_common::fee::Fee::zero();
            c.borrow_interest_rate_strategy =
                templar_common::interest_rate_strategy::InterestRateStrategy::linear(
                    dec!("1000"),
                    dec!("1000"),
                )
                .unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, &market).await?;
    }
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000_000 - 1).await?;

    // Accrue interest until the position becomes (barely) liquidatable.
    harness.fast_forward(200).await?;
    harness
        .apply_interest(&borrow_user, &market, Some(borrow_user.0.clone()), None)
        .await?;

    let (collateral, price) = harness
        .liquidatable_collateral_with_spread(&market, &borrow_user.0)
        .await?;
    assert!(u128::from(collateral) > 0);
    assert!(u128::from(price) > 0);

    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(price),
            Some(u128::from(collateral)),
        )
        .await?;

    let prices = harness.get_oracle_prices(&market).await?;
    assert!(
        !harness
            .get_borrow_status(&market, &borrow_user.0, prices)
            .await?
            .context("borrow status missing")?
            .is_liquidation(),
        "position should be healthy after liquidating all liquidatable collateral",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn liquidators_race(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let (borrow_user, liquidator) = setup_underwater(&harness, &market).await?;
    harness.set_asset_prices(&market, 1.0, 0.5).await?;

    let balance_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let (collateral, price) = harness
        .liquidatable_collateral_with_spread(&market, &borrow_user.0)
        .await?;

    // Two identical liquidations race; only one can succeed (the other tries to
    // take more collateral than is eligible and is refunded).
    let (_a, _b) = tokio::join!(
        harness.try_liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(price),
            Some(u128::from(collateral)),
        ),
        harness.try_liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(price),
            Some(u128::from(collateral)),
        ),
    );

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        balance_before + 500,
        "liquidation should only occur once",
    );

    Ok(())
}
