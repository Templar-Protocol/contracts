use rstest::rstest;

use templar_common::withdrawal_queue::WithdrawalQueueStatus;
use test_utils::*;

#[rstest]
#[tokio::test]
async fn successful_withdrawal() {
    let SetupEverything { c, supply_user, .. } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 10_000).await;

    let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;
    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(
        status,
        WithdrawalQueueStatus {
            depth: 10_000.into(),
            length: 1
        },
    );
    c.execute_next_supply_withdrawal_request(&supply_user).await;
    let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;
    assert_eq!(
        balance_before + 10_000,
        balance_after,
        "Supply user should receive full deposit back"
    );
}

#[rstest]
#[tokio::test]
async fn unsuccessful_withdrawal() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 10_000).await;
    c.collateralize(&borrow_user, 20_000).await;
    c.borrow(&borrow_user, 5_000).await;

    let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;
    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(
        status,
        WithdrawalQueueStatus {
            depth: 10_000.into(),
            length: 1
        },
    );
    c.execute_next_supply_withdrawal_request(&supply_user).await;
    let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;
    assert_eq!(
        balance_before, balance_after,
        "Supply user does not receive anything"
    );

    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(
        status,
        WithdrawalQueueStatus {
            depth: 10_000.into(),
            length: 1
        },
        "Status of queue remains unchanged",
    );
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Attempt to withdraw more than current deposit"]
async fn attempt_to_withdraw_more_than_deposit() {
    let SetupEverything { c, supply_user, .. } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 10_000).await;
    c.create_supply_withdrawal_request(&supply_user, 12_000)
        .await;
}
