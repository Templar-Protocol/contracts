use std::time::Duration;

use near_sdk::{serde_json::json, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use templar_common::withdrawal_queue::WithdrawalQueueStatus;
use test_utils::*;

#[rstest]
#[tokio::test]
async fn successful_withdrawal(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user));

    c.supply_and_harvest_until_activation(&supply_user, 10_000)
        .await;

    let balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
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
    c.execute_next_supply_withdrawal_request(&supply_user, None)
        .await;
    let balance_after = c.borrow_asset.balance_of(supply_user.id()).await;
    assert_eq!(
        balance_before + 10_000,
        balance_after,
        "Supply user should receive full deposit back"
    );
}

#[rstest]
#[tokio::test]
async fn unsuccessful_withdrawal(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000),
        c.collateralize(&borrow_user, 20_000),
    );
    c.borrow(&borrow_user, 5_000).await;

    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;
    let r = c
        .execute_next_supply_withdrawal_request(&supply_user, None)
        .await;

    assert_eq!(r.depth, 5_000.into());
    assert_eq!(r.length, 0);

    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(
        status,
        WithdrawalQueueStatus {
            depth: 5_000.into(),
            length: 1,
        },
    );
    let r = c
        .execute_next_supply_withdrawal_request(&supply_user, None)
        .await;

    assert_eq!(r.depth, 0.into());
    assert_eq!(r.length, 0);
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Attempt to withdraw more than current deposit"]
async fn attempt_to_withdraw_more_than_deposit_incoming(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user));

    c.supply(&supply_user, 10_000).await;
    c.create_supply_withdrawal_request(&supply_user, 12_000)
        .await;
}

#[rstest]
#[tokio::test]
#[should_panic = "Smart contract panicked: Attempt to withdraw more than current deposit"]
async fn attempt_to_withdraw_more_than_deposit(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user));

    c.supply_and_harvest_until_activation(&supply_user, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user, 12_000)
        .await;
}

#[rstest]
#[tokio::test]
async fn partial_fulfillment(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user_1, supply_user_2, borrow_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user_1, 10_000),
        c.supply_and_harvest_until_activation(&supply_user_2, 10_000),
    );

    c.collateralize(&borrow_user, 20_000).await;
    c.borrow(&borrow_user, 2_000).await;

    let balance_1_before = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_before = c.borrow_asset.balance_of(supply_user_2.id()).await;

    c.create_supply_withdrawal_request(&supply_user_1, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user_2, 10_000)
        .await;

    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(status.depth, 20_000.into());
    assert_eq!(status.length, 2);

    // Supply user 1: Fully fulfilled
    c.execute_next_supply_withdrawal_request(&borrow_user, None)
        .await;
    // Supply user 2: Can only withdraw 8000, because 2000 is borrowed
    c.execute_next_supply_withdrawal_request(&borrow_user, None)
        .await;

    let balance_1_after = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_after = c.borrow_asset.balance_of(supply_user_2.id()).await;

    assert_eq!(balance_1_before + 10_000, balance_1_after);
    assert_eq!(balance_2_before + 8_000, balance_2_after);

    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(status.depth, 2_000.into());
    assert_eq!(status.length, 1);

    let position_1_after = c.get_supply_position(supply_user_1.id()).await.unwrap();
    assert_eq!(position_1_after.get_deposit().total(), 0.into());
    let position_2_after = c.get_supply_position(supply_user_2.id()).await.unwrap();
    assert_eq!(position_2_after.get_deposit().total(), 2_000.into());
}

#[rstest]
#[case(1_000)]
#[case(2_500)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Withdrawal amount is outside of allowable range"]
async fn attempt_to_withdraw_outside_configured_range(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] amount: u128,
) {
    setup_test!(
        worker
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (2000, Some(3000)).try_into().unwrap();
            c.supply_withdrawal_range = (2000, Some(2000)).try_into().unwrap();
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 2_500)
        .await;
    c.create_supply_withdrawal_request(&supply_user, amount)
        .await;
}

#[rstest]
#[tokio::test]
async fn supply_withdrawal_after_storage_unregister(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user, supply_user_2));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 20_000),
        c.supply_and_harvest_until_activation(&supply_user_2, 20_000),
    );

    c.create_supply_withdrawal_request(&supply_user_2, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;

    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(status.depth, 20_000.into());
    assert_eq!(status.length, 2);
    eprintln!("Withdrawal queue status: {status:#?}");

    // supply_user_2 deletes his token account
    supply_user_2
        .call(c.borrow_asset.contract().id(), "patch_storage_unregister")
        .args_json(json!({"force": true}))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let position_1_before = c.get_supply_position(supply_user.id()).await.unwrap();
    let position_2_before = c.get_supply_position(supply_user_2.id()).await.unwrap();

    // First one should fail
    let balance_before = c.borrow_asset.balance_of(supply_user_2.id()).await;
    assert_eq!(balance_before, 0);
    let result = c
        .execute_next_supply_withdrawal_request(&supply_user, None)
        .await;
    assert_eq!(result.depth, 10_000.into());
    assert_eq!(result.length, 1);
    let balance_after = c.borrow_asset.balance_of(supply_user_2.id()).await;

    assert_eq!(balance_after, 0, "Should fail to transfer after unregister");

    // Failed but still removed from queue
    let status = c.get_supply_withdrawal_queue_status().await;
    assert_eq!(
        status.depth,
        10_000u128.into(),
        "Request should be removed after unexpected failure",
    );
    assert_eq!(status.length, 1);

    let balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
    let r = c
        .execute_next_supply_withdrawal_request(&supply_user, None)
        .await;
    assert_eq!(r.depth, 10_000.into());
    assert_eq!(r.length, 1);
    let balance_after = c.borrow_asset.balance_of(supply_user.id()).await;
    assert_eq!(balance_before + 10_000, balance_after);
    let status = c.get_supply_withdrawal_queue_status().await;
    assert!(status.depth.is_zero());
    assert_eq!(status.length, 0);

    let position_1_after = c.get_supply_position(supply_user.id()).await.unwrap();
    let position_2_after = c.get_supply_position(supply_user_2.id()).await.unwrap();

    assert_eq!(
        u128::from(position_1_before.get_deposit().total()) - 10_000u128,
        position_1_after.get_deposit().total().into(),
    );
    assert_eq!(
        position_2_before.get_deposit().total(),
        position_2_after.get_deposit().total(),
    );
}

#[rstest]
#[tokio::test]
async fn deposit_during_withdrawal(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user, borrow_user));

    c.supply(&supply_user, 10_000).await;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;

    tokio::join!(
        async {
            let r = c
                .execute_next_supply_withdrawal_request_exec(&supply_user, None)
                .await;
            assert!(r.failures().is_empty());
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let r = c.supply(&supply_user, 1_000).await;
            assert!(r.failures().is_empty());
        },
    );

    let position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(position.get_deposit().total(), 1_000.into());
}

#[rstest]
#[tokio::test]
async fn batch_fulfillment(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user_1, supply_user_2, supply_user_3));

    tokio::join!(
        c.supply(&supply_user_1, 10_000),
        c.supply(&supply_user_2, 10_000),
        c.supply(&supply_user_3, 10_000),
    );

    c.create_supply_withdrawal_request(&supply_user_1, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user_2, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user_3, 10_000)
        .await;

    let balance_1_before = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_before = c.borrow_asset.balance_of(supply_user_2.id()).await;
    let balance_3_before = c.borrow_asset.balance_of(supply_user_3.id()).await;

    let r = c
        .execute_next_supply_withdrawal_request(&supply_user_1, Some(100))
        .await;
    assert_eq!(r.depth, 30_000.into());
    assert_eq!(r.length, 3);

    let balance_1_after = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_after = c.borrow_asset.balance_of(supply_user_2.id()).await;
    let balance_3_after = c.borrow_asset.balance_of(supply_user_3.id()).await;

    assert_eq!(balance_1_before + 10_000, balance_1_after);
    assert_eq!(balance_2_before + 10_000, balance_2_after);
    assert_eq!(balance_3_before + 10_000, balance_3_after);
}

#[rstest]
#[tokio::test]
async fn batch_fulfillment_partial(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user_1, supply_user_2, supply_user_3, borrow_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user_1, 10_000),
        c.supply(&supply_user_2, 10_000),
        c.supply(&supply_user_3, 10_000),
        c.collateralize(&borrow_user, 20_000),
    );

    c.borrow(&borrow_user, 5_000).await;

    c.create_supply_withdrawal_request(&supply_user_1, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user_2, 10_000)
        .await;
    c.create_supply_withdrawal_request(&supply_user_3, 10_000)
        .await;

    let balance_1_before = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_before = c.borrow_asset.balance_of(supply_user_2.id()).await;
    let balance_3_before = c.borrow_asset.balance_of(supply_user_3.id()).await;

    let r = c
        .execute_next_supply_withdrawal_request(&supply_user_1, Some(100))
        .await;

    assert_eq!(r.depth, 25_000.into());
    assert_eq!(r.length, 2);

    let balance_1_after = c.borrow_asset.balance_of(supply_user_1.id()).await;
    let balance_2_after = c.borrow_asset.balance_of(supply_user_2.id()).await;
    let balance_3_after = c.borrow_asset.balance_of(supply_user_3.id()).await;

    assert_eq!(balance_1_before + 10_000, balance_1_after);
    assert_eq!(balance_2_before + 10_000, balance_2_after);
    assert_eq!(balance_3_before + 5_000, balance_3_after);
}

#[rstest]
#[tokio::test]
async fn measure_gas(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(supply_user_1, supply_user_2, supply_user_3, supply_user_4));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user_1, 20_000),
        c.supply_and_harvest_until_activation(&supply_user_2, 20_000),
        c.supply_and_harvest_until_activation(&supply_user_3, 20_000),
        c.supply_and_harvest_until_activation(&supply_user_4, 20_000),
    );

    c.create_supply_withdrawal_request(&supply_user_1, 1_000)
        .await;
    let r = c
        .execute_next_supply_withdrawal_request_exec(&supply_user_1, None)
        .await;

    let r1 = r.total_gas_burnt.as_gas();

    tokio::join!(
        c.create_supply_withdrawal_request(&supply_user_1, 1_000),
        c.create_supply_withdrawal_request(&supply_user_2, 1_000),
        c.create_supply_withdrawal_request(&supply_user_3, 1_000),
        c.create_supply_withdrawal_request(&supply_user_4, 1_000),
    );
    let r = c
        .execute_next_supply_withdrawal_request_exec(&supply_user_1, Some(100))
        .await;

    let r2 = r.total_gas_burnt.as_gas();

    let base = near_sdk::Gas::from_gas((4 * r1 - r2) / 3);
    let amount_per_request = near_sdk::Gas::from_gas((r2 - r1) / 3);

    eprintln!("Base: {base}");
    eprintln!("Amount per request: {amount_per_request}");

    assert!(base < near_sdk::Gas::from_tgas(7));
    assert!(amount_per_request < near_sdk::Gas::from_tgas(5));
}
