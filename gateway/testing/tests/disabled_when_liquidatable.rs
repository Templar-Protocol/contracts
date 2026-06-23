//! Ported from `contract/market/tests/disabled_when_liquidatable.rs`: while a
//! position is liquidatable, only actions that cure it are allowed. The rejected
//! actions go through `ft_transfer_call`, so the contract's panic is caught by
//! the FT and refunded — asserted here as "no effect".

use anyhow::{Context, Result};
use rstest::rstest;
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};

/// Set up a borrow that becomes liquidatable when the collateral price halves.
async fn liquidatable_position(
    harness: &SandboxHarness,
) -> Result<(DeployedMarket, templar_gateway_types::ManagedAccountId)> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    harness.borrow(&borrow_user, &market, 1_000_000).await?;

    // Halve the collateral value → the position is now liquidatable.
    harness.set_asset_prices(&market, 1.0, 0.5).await?;
    Ok((market, borrow_user))
}

async fn collateral_deposit(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    account: &templar_gateway_types::ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_borrow_position(market, &account.0)
            .await?
            .context("borrow position missing")?
            .collateral_asset_deposit,
    ))
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn disallow_insufficient_collateralization_while_liquidatable(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (market, borrow_user) = liquidatable_position(&harness).await?;

    let before = collateral_deposit(&harness, &market, &borrow_user).await?;
    // A tiny top-up that leaves the position liquidatable must be rejected.
    harness
        .try_collateralize(&borrow_user, &market, 2_000)
        .await?;
    let after = collateral_deposit(&harness, &market, &borrow_user).await?;
    assert_eq!(
        before, after,
        "adding collateral that leaves the position liquidatable must be rejected",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn allow_sufficient_collateralization_while_liquidatable(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (market, borrow_user) = liquidatable_position(&harness).await?;

    let before = collateral_deposit(&harness, &market, &borrow_user).await?;
    // A top-up that cures the liquidation is allowed.
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;
    let after = collateral_deposit(&harness, &market, &borrow_user).await?;
    assert_eq!(
        before + 2_000_000,
        after,
        "collateralization that brings the position out of liquidation should be allowed",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn disallow_repayment_while_liquidatable(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let (market, borrow_user) = liquidatable_position(&harness).await?;

    let liability_before = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .get_total_borrow_asset_liability(),
    );
    harness
        .try_repay(&borrow_user, &market, 1_050_000, None)
        .await?;
    let liability_after = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .get_total_borrow_asset_liability(),
    );
    assert!(
        liability_after >= liability_before,
        "repayment must be rejected while the position is liquidatable",
    );

    Ok(())
}
