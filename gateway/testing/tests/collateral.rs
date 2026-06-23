//! Ported from `contract/market/tests/collateral.rs` onto the gateway harness.
//!
//! The original `collateral_withdrawal` raced 30 concurrent withdrawals against
//! a storage unregistration, asserting at least one withdrawal failed. Here we
//! recreate the same *failure condition* deterministically: a collateral
//! withdrawal cannot succeed once the account is unregistered from the
//! collateral token, and the collateral is not lost.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn collateral_withdrawal_blocked_by_storage_unregistration(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness.collateralize(&borrow_user, &market, 2000).await?;

    // A withdrawal works while the account is registered on the collateral token.
    harness
        .withdraw_collateral(&borrow_user, &market, 1000)
        .await?;
    let deposit_after_first = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .collateral_asset_deposit,
    );
    assert_eq!(deposit_after_first, 1000);

    // Unregister from the collateral token: the market can no longer return
    // collateral to this account.
    harness
        .storage_unregister(&borrow_user, &market.collateral_ft_id, true)
        .await?;

    // The next withdrawal cannot move collateral out — the remaining deposit is
    // left intact (not lost) whether the op fails outright or reverts/refunds.
    harness
        .try_withdraw_collateral(&borrow_user, &market, 1000)
        .await?;
    let deposit_after_blocked = u128::from(
        harness
            .get_borrow_position(&market, &borrow_user.0)
            .await?
            .context("borrow position missing")?
            .collateral_asset_deposit,
    );
    assert_eq!(
        deposit_after_blocked, 1000,
        "collateral must not be lost when withdrawal to an unregistered account is blocked",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn excessive_collateral_withdrawal_is_rejected(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let borrow_user_1 = harness.create_user("borrow1").await?;
    let borrow_user_2 = harness.create_user("borrow2").await?;
    harness.fund_user(&borrow_user_1, &market).await?;
    harness.fund_user(&borrow_user_2, &market).await?;

    harness
        .collateralize(&borrow_user_1, &market, 1_000_000)
        .await?;
    harness
        .collateralize(&borrow_user_2, &market, 1_000_000)
        .await?;

    // Withdrawing more collateral than deposited must be rejected (an unsigned
    // underflow), leaving the deposit intact.
    let result = harness
        .try_withdraw_collateral(&borrow_user_1, &market, 1_000_000 + 1)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("attempt to subtract with overflow"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );

    let deposit = harness
        .get_borrow_position(&market, &borrow_user_1.0)
        .await?
        .context("borrow position missing")?
        .collateral_asset_deposit;
    assert_eq!(u128::from(deposit), 1_000_000);

    Ok(())
}
