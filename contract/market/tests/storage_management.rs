//! The original asserts that supplying from an account not registered on the
//! market panics ("is not registered"). The gateway `supply` op auto-registers
//! the signer, so to exercise the contract's own requirement we bypass it with a
//! raw `ft_transfer_call` carrying the `Supply` message. The same *failure
//! condition* surfaces as an effect: the market rejects the deposit inside
//! `ft_on_transfer`, the FT refunds, and no supply position is created.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::market::DepositMsg;
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

#[rstest]
#[tokio::test]
async fn supply_requires_market_registration(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    // `fund_user` registers the account on the tokens and mints it a balance, but
    // does not register it on the market — that registration is what the gateway
    // `supply` op would add, and what we deliberately skip here.
    let user = harness.create_user("supply").await?;
    harness.fund_user(&user, &market).await?;

    let balance_before = harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?;
    let result = harness
        .ft_transfer_call(
            &user,
            &market.borrow_ft_id,
            &market.market_id,
            1000,
            serde_json::to_string(&DepositMsg::Supply)?,
        )
        .await?;

    // The market rejects the deposit specifically because the account is not
    // registered; the FT then refunds (so the operation reports Failed).
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("is not registered"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );
    assert!(
        harness
            .get_supply_position(&market, &user.0)
            .await?
            .is_none(),
        "an unregistered account must not get a supply position",
    );
    assert_eq!(
        harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?,
        balance_before,
        "the rejected deposit must be refunded",
    );

    Ok(())
}

/// Regression: a signer who supplies first — registering on the market at its
/// minimum, then spending that minimum on the supply position — must still be
/// able to collateralize through the gateway. Collateralizing opens a *borrow*
/// position the market charges storage for again, out of the same balance, so the
/// `collateralize` plan has to top the available balance back up to the minimum;
/// merely being registered is not enough, and the deposit would otherwise panic
/// with a storage error inside `execute_collateralize`.
#[rstest]
#[tokio::test]
async fn supply_then_collateralize_by_same_account(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;

    let user = harness.create_user("supplier-borrower").await?;
    harness.fund_user(&user, &market).await?;

    harness.supply(&user, &market, 1000).await?;
    assert!(
        harness
            .get_supply_position(&market, &user.0)
            .await?
            .is_some(),
        "supplying should create a supply position",
    );

    harness.collateralize(&user, &market, 2000).await?;
    let borrow_position = harness
        .get_borrow_position(&market, &user.0)
        .await?
        .context("collateralizing after supplying should create a borrow position")?;
    assert_eq!(
        u128::from(borrow_position.collateral_asset_deposit),
        2000,
        "the collateral must be recorded despite the account already holding a supply position",
    );

    Ok(())
}
