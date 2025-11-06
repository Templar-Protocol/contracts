use std::time::Duration;

use near_sdk::{serde_json::json, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use templar_common::{asset::BorrowAssetAmount, dec, interest_rate_strategy::InterestRateStrategy};
use test_utils::*;

#[rstest]
#[tokio::test]
async fn static_yield_success(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c, protocol_yield_user, insurance_yield_user)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    let record_before = c.get_static_yield(protocol_yield_user.id()).await;
    assert_eq!(record_before, None);

    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;

    let record_after_noop_accumulate = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();
    assert_eq!(record_after_noop_accumulate, 0.into());

    c.borrow(&borrow_user, 1_000_000).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    c.repay(&borrow_user, 1_200_000).await;

    let record_after_repay = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();
    assert_eq!(record_after_repay, 0.into());

    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;

    let record_after_accumulate = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();
    assert_ne!(record_after_accumulate, 0.into());

    c.accumulate_static_yield(
        &protocol_yield_user,
        Some(insurance_yield_user.id().clone()),
        None,
    )
    .await;

    // Insurance user hasn't done anything yet
    let second_record_after_accumulate = c
        .get_static_yield(insurance_yield_user.id())
        .await
        .unwrap()
        .get_total();
    assert!(second_record_after_accumulate >= record_after_accumulate);

    let balance_before = c.borrow_asset.balance_of(protocol_yield_user.id()).await;

    // Ensure withdrawing works properly
    c.withdraw_static_yield(&protocol_yield_user, Some(1.into()))
        .await;

    let balance_after_withdraw_1 = c.borrow_asset.balance_of(protocol_yield_user.id()).await;

    assert_eq!(balance_before + 1, balance_after_withdraw_1);

    let record_after_withdraw_1 = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();

    assert_eq!(
        record_after_withdraw_1 + BorrowAssetAmount::from(1),
        record_after_accumulate,
    );

    // Withdraw all
    c.withdraw_static_yield(&protocol_yield_user, None).await;

    let record_after_withdraw_all = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();

    assert_eq!(record_after_withdraw_all, 0.into());

    let balance_after_withdraw_all = c.borrow_asset.balance_of(protocol_yield_user.id()).await;

    assert_eq!(
        balance_before + u128::from(record_after_accumulate),
        balance_after_withdraw_all,
    );
}

#[rstest]
#[tokio::test]
async fn static_yield_fail_storage_unregistered(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c, protocol_yield_user, insurance_yield_user)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 10_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    c.borrow(&borrow_user, 1_000_000).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    c.repay(&borrow_user, 1_200_000).await;

    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;

    let record_after_accumulate = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();
    eprintln!("Record after accumulate: {record_after_accumulate}");
    assert_ne!(record_after_accumulate, 0.into());

    let r = protocol_yield_user
        .call(c.borrow_asset.contract().id(), "patch_storage_unregister")
        .args_json(json!({"force": true}))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await
        .unwrap()
        .into_result()
        .unwrap();

    eprintln!("Storage unregister: {r:?}");

    let r = c.withdraw_static_yield(&protocol_yield_user, None).await;
    eprintln!("Withdraw static yield: {r:?}");

    let record_after_failed_withdrawal = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total();
    eprintln!("Record after failed withdrawal: {record_after_failed_withdrawal}");
    assert_eq!(record_after_failed_withdrawal, record_after_accumulate);
}
