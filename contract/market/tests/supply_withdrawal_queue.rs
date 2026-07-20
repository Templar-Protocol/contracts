//! The original also asserts on the `WithdrawalQueueStatus` *returned* by
//! `execute_next_supply_withdrawal_request`; the gateway write op surfaces only
//! the transaction result, so here we assert the same outcomes via the queue
//! status *read* plus balances/positions. Gas is read via the harness
//! `operation_gas_burnt` helper. The mock `patch_storage_unregister` is replaced
//! with the gateway `storage::unregister`.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_gateway_testing::{failed_receipts, harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::{ManagedAccountId, OperationStatus};

const OUT_OF_RANGE: &str = "Withdrawal amount is outside of allowable range";
const MORE_THAN_DEPOSIT: &str = "Attempt to withdraw more than current deposit";

async fn queue(harness: &SandboxHarness, market: &DeployedMarket) -> Result<(u128, u32)> {
    let status = harness.supply_withdrawal_queue_status(market).await?;
    Ok((u128::from(status.depth), status.length))
}

async fn deposit_total(
    harness: &SandboxHarness,
    market: &DeployedMarket,
    user: &ManagedAccountId,
) -> Result<u128> {
    Ok(u128::from(
        harness
            .get_supply_position(market, &user.0)
            .await?
            .context("supply position missing")?
            .get_deposit()
            .total(),
    ))
}

#[rstest]
#[tokio::test]
async fn successful_withdrawal(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 10_000)
        .await?;
    assert_eq!(queue(&harness, &market).await?, (10_000, 1));

    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before + 10_000,
    );
    assert_eq!(queue(&harness, &market).await?, (0, 0));

    Ok(())
}

#[rstest]
#[tokio::test]
async fn partial_fulfillment_when_liquidity_insufficient(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&borrow_user, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 20_000).await?;
    harness.borrow(&borrow_user, &market, 5_000).await?;

    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 10_000)
        .await?;
    // Only the 5_000 of unborrowed liquidity can be withdrawn; the rest stays queued.
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(queue(&harness, &market).await?, (5_000, 1));
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before + 5_000,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn reject_withdraw_more_than_incoming_deposit(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;
    // Not harvested: the deposit is still "incoming".
    harness.supply(&supply_user, &market, 10_000).await?;

    let result = harness
        .try_create_supply_withdrawal_request(&supply_user, &market, 12_000)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(result
        .operation
        .failure_message()
        .unwrap_or_default()
        .contains(MORE_THAN_DEPOSIT));

    Ok(())
}

#[rstest]
#[tokio::test]
async fn reject_withdraw_more_than_active_deposit(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000)
        .await?;

    let result = harness
        .try_create_supply_withdrawal_request(&supply_user, &market, 12_000)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(result
        .operation
        .failure_message()
        .unwrap_or_default()
        .contains(MORE_THAN_DEPOSIT));

    Ok(())
}

#[rstest]
#[case(1_000)]
#[case(2_500)]
#[tokio::test]
async fn reject_withdraw_outside_configured_range(
    #[future(awt)] harness: SandboxHarness,
    #[case] amount: u128,
) -> Result<()> {
    let market = harness
        .deploy_full_market_with(|c| {
            c.supply_range = (2000, Some(3000)).try_into().unwrap();
            c.supply_withdrawal_range = (2000, Some(2000)).try_into().unwrap();
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 2_500)
        .await?;

    let result = harness
        .try_create_supply_withdrawal_request(&supply_user, &market, amount)
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(result
        .operation
        .failure_message()
        .unwrap_or_default()
        .contains(OUT_OF_RANGE));

    Ok(())
}

#[rstest]
#[tokio::test]
async fn partial_fulfillment_across_two_suppliers(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_1 = harness.create_user("supply1").await?;
    let supply_2 = harness.create_user("supply2").await?;
    let borrow_user = harness.create_user("borrow").await?;
    for user in [&supply_1, &supply_2, &borrow_user] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_1, &market, 10_000)
        .await?;
    harness
        .supply_and_harvest_until_activation(&supply_2, &market, 10_000)
        .await?;
    harness.collateralize(&borrow_user, &market, 20_000).await?;
    harness.borrow(&borrow_user, &market, 2_000).await?;

    let balance_1_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_1.0)
        .await?;
    let balance_2_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_2.0)
        .await?;

    harness
        .create_supply_withdrawal_request(&supply_1, &market, 10_000)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_2, &market, 10_000)
        .await?;
    assert_eq!(queue(&harness, &market).await?, (20_000, 2));

    // First fully fulfilled; second can only get 8_000 (2_000 is borrowed).
    harness
        .execute_next_supply_withdrawal_request(&borrow_user, &market, None)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&borrow_user, &market, None)
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_1.0)
            .await?,
        balance_1_before + 10_000,
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_2.0)
            .await?,
        balance_2_before + 8_000,
    );
    assert_eq!(queue(&harness, &market).await?, (2_000, 1));
    assert_eq!(deposit_total(&harness, &market, &supply_1).await?, 0);
    assert_eq!(deposit_total(&harness, &market, &supply_2).await?, 2_000);

    Ok(())
}

#[rstest]
#[tokio::test]
async fn failed_transfer_still_dequeues(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let supply_2 = harness.create_user("supply2").await?;
    harness.fund_user(&supply_user, &market).await?;
    harness.fund_user(&supply_2, &market).await?;

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 20_000)
        .await?;
    harness
        .supply_and_harvest_until_activation(&supply_2, &market, 20_000)
        .await?;

    // supply_2 is enqueued first, then supply_user.
    harness
        .create_supply_withdrawal_request(&supply_2, &market, 10_000)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 10_000)
        .await?;
    assert_eq!(queue(&harness, &market).await?, (20_000, 2));

    let position_1_before = deposit_total(&harness, &market, &supply_user).await?;
    let position_2_before = deposit_total(&harness, &market, &supply_2).await?;

    // supply_2 unregisters from the borrow token so its transfer cannot land.
    harness
        .storage_unregister(&supply_2, &market.borrow_ft_id, true)
        .await?;

    // First execution targets supply_2: the transfer fails, but the request is
    // still removed from the queue.
    // The payout transfer to the unregistered supply_2 fails, so this is a failed
    // receipt under an overall-successful transaction — the strict path would
    // reject it. Assert the failure explicitly: if the transfer ever *stopped*
    // failing, the balance check below would still pass (supply_2 would just be
    // paid), and the test would silently stop covering the dequeue-on-failure
    // path it exists for.
    let result = harness
        .try_execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        result.operation.status,
        OperationStatus::Succeeded,
        "dequeuing must succeed at top level even when the payout transfer fails",
    );
    assert!(
        failed_receipts(&result).any(|receipt| receipt.contract_id == market.borrow_ft_id),
        "expected the payout transfer to the unregistered account to fail",
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_2.0)
            .await?,
        0
    );
    assert_eq!(queue(&harness, &market).await?, (10_000, 1));

    // Second execution fulfills supply_user normally.
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_user.0)
            .await?,
        balance_before + 10_000,
    );
    assert_eq!(queue(&harness, &market).await?, (0, 0));

    assert_eq!(
        deposit_total(&harness, &market, &supply_user).await?,
        position_1_before - 10_000,
    );
    // supply_2's failed withdrawal leaves its position unchanged.
    assert_eq!(
        deposit_total(&harness, &market, &supply_2).await?,
        position_2_before,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn deposit_during_withdrawal(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    harness.fund_user(&supply_user, &market).await?;

    harness.supply(&supply_user, &market, 10_000).await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 10_000)
        .await?;

    // A deposit landing *during* the in-flight withdrawal must be retained, not
    // consumed by it. Order the two so the withdrawal is submitted first and the
    // supply arrives while it is executing — without the delay the join could
    // pass trivially on runs where the supply happens to land first. Both use the
    // non-`try` wrappers, which already assert each side succeeded.
    let (execute, supply) = tokio::join!(
        harness.execute_next_supply_withdrawal_request(&supply_user, &market, None),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            harness.supply(&supply_user, &market, 1_000).await
        },
    );
    execute?;
    supply?;

    assert_eq!(deposit_total(&harness, &market, &supply_user).await?, 1_000);

    Ok(())
}

#[rstest]
#[tokio::test]
async fn batch_fulfillment(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_1 = harness.create_user("supply1").await?;
    let supply_2 = harness.create_user("supply2").await?;
    let supply_3 = harness.create_user("supply3").await?;
    for user in [&supply_1, &supply_2, &supply_3] {
        harness.fund_user(user, &market).await?;
        harness.supply(user, &market, 10_000).await?;
    }

    for user in [&supply_1, &supply_2, &supply_3] {
        harness
            .create_supply_withdrawal_request(user, &market, 10_000)
            .await?;
    }

    let before: Vec<u128> = {
        let mut v = vec![];
        for user in [&supply_1, &supply_2, &supply_3] {
            v.push(harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?);
        }
        v
    };

    // A single batched execution fulfills all three requests.
    harness
        .execute_next_supply_withdrawal_request(&supply_1, &market, Some(100))
        .await?;

    for (user, before) in [&supply_1, &supply_2, &supply_3].iter().zip(before) {
        assert_eq!(
            harness.ft_balance_of(&market.borrow_ft_id, &user.0).await?,
            before + 10_000,
        );
    }
    assert_eq!(queue(&harness, &market).await?, (0, 0));

    Ok(())
}

#[rstest]
#[tokio::test]
async fn batch_fulfillment_partial(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_1 = harness.create_user("supply1").await?;
    let supply_2 = harness.create_user("supply2").await?;
    let supply_3 = harness.create_user("supply3").await?;
    let borrow_user = harness.create_user("borrow").await?;
    for user in [&supply_1, &supply_2, &supply_3, &borrow_user] {
        harness.fund_user(user, &market).await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_1, &market, 10_000)
        .await?;
    harness.supply(&supply_2, &market, 10_000).await?;
    harness.supply(&supply_3, &market, 10_000).await?;
    harness.collateralize(&borrow_user, &market, 20_000).await?;
    harness.borrow(&borrow_user, &market, 5_000).await?;

    for user in [&supply_1, &supply_2, &supply_3] {
        harness
            .create_supply_withdrawal_request(user, &market, 10_000)
            .await?;
    }

    let b1 = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_1.0)
        .await?;
    let b2 = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_2.0)
        .await?;
    let b3 = harness
        .ft_balance_of(&market.borrow_ft_id, &supply_3.0)
        .await?;

    harness
        .execute_next_supply_withdrawal_request(&supply_1, &market, Some(100))
        .await?;

    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_1.0)
            .await?,
        b1 + 10_000,
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_2.0)
            .await?,
        b2 + 10_000,
    );
    // 5_000 is borrowed, so the third supplier is only partially fulfilled.
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &supply_3.0)
            .await?,
        b3 + 5_000,
    );
    // ...and its unpaid 5_000 remains queued as a single request — payouts alone
    // cannot detect a partially-paid request being dropped from the queue.
    assert_eq!(
        queue(&harness, &market).await?,
        (5_000, 1),
        "the partially-fulfilled request must remain queued for its unpaid remainder",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn measure_gas(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    const TGAS: u64 = 1_000_000_000_000;

    let market = harness.deploy_full_market().await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let users = [
        harness.create_user("supply1").await?,
        harness.create_user("supply2").await?,
        harness.create_user("supply3").await?,
        harness.create_user("supply4").await?,
    ];
    for user in &users {
        harness.fund_user(user, &market).await?;
    }
    for user in &users {
        harness
            .supply_and_harvest_until_activation(user, &market, 20_000)
            .await?;
    }

    // Gas for fulfilling a single request.
    harness
        .create_supply_withdrawal_request(&users[0], &market, 1_000)
        .await?;
    let one = harness.operation_gas_burnt(
        &harness
            .execute_next_supply_withdrawal_request(&users[0], &market, None)
            .await?,
    );

    // Gas for fulfilling four requests in one batch.
    for user in &users {
        harness
            .create_supply_withdrawal_request(user, &market, 1_000)
            .await?;
    }
    let four = harness.operation_gas_burnt(
        &harness
            .execute_next_supply_withdrawal_request(&users[0], &market, Some(100))
            .await?,
    );

    // one = base + 1*per_request, four = base + 4*per_request.
    let base = (4 * one).saturating_sub(four) / 3;
    let per_request = four.saturating_sub(one) / 3;

    assert!(base < 7 * TGAS, "base gas too high: {base}");
    assert!(
        per_request < 5 * TGAS,
        "per-request gas too high: {per_request}",
    );

    Ok(())
}
