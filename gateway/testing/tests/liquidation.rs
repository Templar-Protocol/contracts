//! Ported from `contract/market/tests/liquidation.rs`.
//!
//! Rejected liquidations pay via `ft_transfer_call`, so the contract's rejection
//! is refunded and asserted here as "no effect" (balances + position unchanged).
//! `liquidatable_collateral_fmv`/`_with_spread` are harness helpers mirroring the
//! retired controller. The interest-only case advances time with `fast_forward`
//! instead of sleeping; the extreme-price cases set raw pyth prices with explicit
//! exponents.

use anyhow::{Context, Result};
use near_token::NearToken;
use rstest::rstest;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, market::YieldWeights,
    oracle::pyth, Decimal,
};
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

fn price(value: i64, exponent: i32) -> pyth::Price {
    pyth::Price {
        price: value.into(),
        conf: 0.into(),
        expo: exponent,
        publish_time: pyth::PythTimestamp::from_secs(0),
    }
}

#[rstest]
#[case((10, 1000), (10, 1000), (9, 1000), (10, 1000))]
#[case((10, -1000), (10, -1000), (9, -1000), (10, -1000))]
#[case((10, 1000), (10, 1000), (90, 999), (10, 1000))]
#[case((10, 1000), (10, 1000), (10, 1000), (11, 1000))]
#[case((10, 1000), (10, 1000), (10, -1000), (10, 1000))]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn extreme_prices(
    #[future(awt)] harness: SandboxHarness,
    #[case] (collateral_price, collateral_expo): (i64, i32),
    #[case] (borrow_price, borrow_expo): (i64, i32),
    #[case] (new_collateral_price, new_collateral_expo): (i64, i32),
    #[case] (new_borrow_price, new_borrow_expo): (i64, i32),
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
        })
        .await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .set_collateral_asset_price_exact(&market, Some(price(collateral_price, collateral_expo)))
        .await?;
    harness
        .set_borrow_asset_price_exact(&market, Some(price(borrow_price, borrow_expo)))
        .await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_000_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 2000).await?;
    harness.borrow(&borrow_user, &market, 1000).await?;

    let position_before = harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .context("borrow position missing")?;
    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    harness
        .set_collateral_asset_price_exact(
            &market,
            Some(price(new_collateral_price, new_collateral_expo)),
        )
        .await?;
    harness
        .set_borrow_asset_price_exact(&market, Some(price(new_borrow_price, new_borrow_expo)))
        .await?;

    let (collateral, pay) = harness
        .liquidatable_collateral_with_spread(&market, &borrow_user.0)
        .await?;
    assert!(u128::from(collateral) > 0);
    assert!(u128::from(pay) > 0);

    // Offer one less than required: rejected and refunded.
    harness
        .try_liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(pay) - 1,
            Some(u128::from(collateral)),
        )
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

    // Offer enough: succeeds.
    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            u128::from(pay),
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
        borrow_before - u128::from(pay),
    );

    let position_after = harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .context("borrow position missing")?;
    assert_eq!(
        position_before.get_total_collateral_amount()
            - position_after.get_total_collateral_amount(),
        collateral,
    );
    assert_eq!(
        position_before.get_total_borrow_asset_liability()
            - position_after.get_total_borrow_asset_liability(),
        pay,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn partial_liquidation(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.liquidation_maximum_spread = dec!("0.05");
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let alice = harness.create_user("alice").await?;
    let bob = harness.create_user("bob").await?;
    for user in [&supply_user, &borrow_user, &alice, &bob] {
        harness.fund_user(user, &market).await?;
    }

    let compensate_initial_fee = (100_000u128
        * market.configuration.single_snapshot_maximum_interest())
    .to_u128_ceil()
    .context("fee overflow")?
        * 2;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 200_000 + compensate_initial_fee)
        .await?;
    harness.borrow(&borrow_user, &market, 100_000).await?;

    // Raise the borrow-asset price so the position is liquidatable.
    harness.set_asset_prices(&market, 1.5, 1.0).await?;
    let (collateral, pay) = harness
        .liquidatable_collateral_fmv(&market, &borrow_user.0)
        .await?;

    let alice_collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &alice.0)
        .await?;
    let alice_borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &alice.0)
        .await?;
    let bob_collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &bob.0)
        .await?;
    let bob_borrow_before = harness.ft_balance_of(&market.borrow_ft_id, &bob.0).await?;

    // Alice fully liquidates the eligible portion.
    harness
        .liquidate(
            &alice,
            &market,
            &borrow_user.0,
            u128::from(pay),
            Some(u128::from(collateral)),
        )
        .await?;
    // Bob is too late — the position is healthy again, so his attempt is refunded.
    harness
        .try_liquidate(
            &bob,
            &market,
            &borrow_user.0,
            u128::from(pay),
            Some(u128::from(collateral)),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &alice.0)
            .await?,
        alice_collateral_before + u128::from(collateral),
        "Alice receives collateral",
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &alice.0)
            .await?,
        alice_borrow_before - u128::from(pay),
        "Alice pays for collateral",
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &bob.0)
            .await?,
        bob_collateral_before,
        "Bob receives no collateral",
    );
    assert_eq!(
        harness.ft_balance_of(&market.borrow_ft_id, &bob.0).await?,
        bob_borrow_before,
        "Bob is refunded",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn partial_liquidation_fail_offer_too_little(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 5.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 150_000)
        .await?;
    harness.borrow(&borrow_user, &market, 100_000).await?;

    // Collateral value collapses -> liquidatable.
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    // Offering only 10_000 for 50_000 collateral is too low: rejected and refunded.
    harness
        .try_liquidate(&liquidator, &market, &borrow_user.0, 10_000, Some(50_000))
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

    let prices = harness.get_oracle_prices(&market).await?;
    assert!(harness
        .get_borrow_status(&market, &borrow_user.0, prices)
        .await?
        .context("borrow status missing")?
        .is_liquidation());

    Ok(())
}

#[rstest]
#[case(&[dec!("0.5"), dec!("0.49")])]
#[case(&[dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.1"), dec!("0.096")])]
#[case(&[dec!("0.5"), dec!("0.25"), dec!("0.125"), dec!("0.0625"), dec!("0.06235")])]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn many_little_partial_liquidations(
    #[future(awt)] harness: SandboxHarness,
    #[case] pattern: &[Decimal],
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.borrow_mcr_liquidation = dec!("2");
            c.borrow_mcr_maintenance = dec!("2");
            c.borrow_origination_fee = Fee::zero();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 5.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [&supply_user, &borrow_user, &liquidator] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 1_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 150_000)
        .await?;
    harness.borrow(&borrow_user, &market, 100_000).await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    let (collateral, pay) = harness
        .liquidatable_collateral_with_spread(&market, &borrow_user.0)
        .await?;
    let collateral = u128::from(collateral);
    let pay = u128::from(pay);

    let mut total_collateral = 0;
    let mut total_paid = 0;
    for fraction in pattern {
        let collateral_fraction = (collateral * *fraction)
            .to_u128_floor()
            .context("overflow")?;
        let price_fraction = (pay * *fraction).to_u128_ceil().context("overflow")?;
        harness
            .liquidate(
                &liquidator,
                &market,
                &borrow_user.0,
                price_fraction,
                Some(collateral_fraction),
            )
            .await?;
        total_collateral += collateral_fraction;
        total_paid += price_fraction;
    }

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before + total_collateral,
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before - total_paid,
    );

    Ok(())
}

#[rstest]
#[case(110, 5000, 2450, 50, dec!("1"))]
#[case(120, 1250, 1000, 88, dec!("1"))]
#[case(120, 1250, 1000, 88, dec!(".973"))]
#[case(120, 1250, 1000, 88, dec!(".95"))]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn successful_liquidation_good_debt_under_mcr(
    #[future(awt)] harness: SandboxHarness,
    #[case] mcr: u16,
    #[case] collateral_amount: u128,
    #[case] borrow_amount: u128,
    #[case] collateral_price_pct: u128,
    #[case] fmv_frac: Decimal,
) -> Result<()> {
    let protocol = harness.create_user("protocol").await?;
    let insurance = harness.create_user("insurance").await?;
    let protocol_id = protocol.0.clone();
    let insurance_id = insurance.0.clone();
    let mcr_dec = Decimal::from(mcr) / 100u32;
    let market = harness
        .deploy_full_market_with(move |c| {
            c.borrow_origination_fee = Fee::Flat(10.into());
            c.borrow_mcr_liquidation = mcr_dec;
            c.borrow_mcr_maintenance = mcr_dec;
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
            c.yield_weights = YieldWeights::new_with_supply_weight(8)
                .with_static(protocol_id, 1)
                .with_static(insurance_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    let liquidator = harness.create_user("liquidator").await?;
    for user in [
        &protocol,
        &insurance,
        &supply_user,
        &borrow_user,
        &liquidator,
    ] {
        harness.fund_user(user, &market).await?;
    }
    for user in [&protocol, &insurance] {
        harness
            .storage_deposit(user, &market.market_id, NearToken::from_millinear(50))
            .await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, collateral_amount)
        .await?;
    harness.borrow(&borrow_user, &market, borrow_amount).await?;

    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .fees
        ),
        10,
    );

    let collateral_before = harness
        .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
        .await?;
    let borrow_before = harness
        .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
        .await?;

    harness
        .set_asset_prices(&market, 1.0, (collateral_price_pct as f64) / 100.0)
        .await?;
    let (liquidate, fmv_price) = harness
        .liquidatable_collateral_fmv(&market, &borrow_user.0)
        .await?;
    let price = (u128::from(fmv_price) * fmv_frac)
        .to_u128_ceil()
        .context("price overflow")?;

    harness
        .liquidate(
            &liquidator,
            &market,
            &borrow_user.0,
            price,
            Some(u128::from(liquidate)),
        )
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.collateral_ft_id, &liquidator.0)
            .await?,
        collateral_before + u128::from(liquidate),
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &liquidator.0)
            .await?,
        borrow_before - price,
    );

    let yield_amount = price.saturating_sub(borrow_amount).max(10);

    // Finalize a snapshot so the yield is realizable.
    harness
        .apply_interest(&borrow_user, &market, Some(borrow_user.0.clone()), None)
        .await?;

    // Supply gets 80%, protocol and insurance 10% each.
    harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .borrow_asset_yield
                .get_total()
        ),
        yield_amount * 8 / 10,
    );

    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    assert_eq!(
        harness.static_yield_total(&market, &protocol.0).await?,
        yield_amount / 10,
    );
    harness
        .accumulate_static_yield(&insurance, &market, None, None)
        .await?;
    assert_eq!(
        harness.static_yield_total(&market, &insurance.0).await?,
        yield_amount / 10,
    );

    if u128::from(liquidate) == collateral_amount {
        // 100% liquidated -> position deleted.
        assert!(harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .is_none());
    } else {
        let prices = harness.get_oracle_prices(&market).await?;
        assert!(harness
            .get_borrow_status(&market, &borrow_user.0, prices)
            .await?
            .context("borrow status missing")?
            .is_healthy());
    }

    Ok(())
}
