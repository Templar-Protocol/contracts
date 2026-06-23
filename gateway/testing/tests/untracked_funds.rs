//! Ported from `contract/market/tests/untracked_funds.rs` onto the gateway
//! harness. The old `#[should_panic]` borrow case asserts on the observable
//! effect (no funds disbursed) rather than the panic string, since the gateway
//! reports a failed operation without surfacing the contract message.

use anyhow::Result;
use rstest::rstest;
use templar_gateway_testing::{harness, SandboxHarness};
use templar_gateway_types::OperationStatus;

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn cannot_borrow_untracked_funds(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    // Send borrow asset to the market directly — untracked liquidity that must
    // not be borrowable.
    harness
        .ft_transfer(
            &supply_user,
            &market.borrow_ft_id,
            &market.market_id,
            10_000,
        )
        .await?;
    harness.collateralize(&borrow_user, &market, 20_000).await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    let result = harness.try_borrow(&borrow_user, &market, 12_000).await?;
    assert_eq!(
        result.operation.status,
        OperationStatus::Failed,
        "borrow beyond tracked liquidity must be rejected",
    );
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("Insufficient borrow asset available"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );
    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &borrow_user.0)
        .await?;
    assert_eq!(
        balance_before, balance_after,
        "no funds should be disbursed on a rejected borrow",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
#[ignore = "requires NEAR sandbox"]
async fn cannot_withdraw_untracked_funds(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness
        .ft_transfer(&supply_user, &market.borrow_ft_id, &market.market_id, 8_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 20_000).await?;
    harness.borrow(&borrow_user, &market, 8_000).await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 10_000)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    let balance_after = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    // Only the 2_000 not lent out is available; the 8_000 borrowed stays queued.
    assert_eq!(balance_before + 2_000, balance_after);

    let queue_status = harness.supply_withdrawal_queue_status(&market).await?;
    assert_eq!(queue_status.depth, 8_000u128.into());
    assert_eq!(queue_status.length, 1);

    let request_status = harness
        .supply_withdrawal_request_status(&market, &supply_user.0)
        .await?
        .expect("withdrawal request status");
    assert_eq!(request_status.amount, 8_000u128.into());
    assert_eq!(request_status.depth, 0u128.into());
    assert_eq!(request_status.index, 0);

    Ok(())
}
