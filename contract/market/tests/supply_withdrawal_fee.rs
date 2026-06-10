use std::time::Duration;

use near_sdk::json_types::U64;
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use templar_common::fee::{Fee, TimeBasedFee, TimeBasedFeeFunction};
use test_utils::*;

#[rstest]
#[tokio::test]
async fn supply_withdrawal_fee_flat(#[future(awt)] worker: Worker<Sandbox>) {
    let fee = TimeBasedFee {
        fee: Fee::Flat(100.into()),
        duration: U64(1000 * 60 * 60 * 24 * 30),
        behavior: TimeBasedFeeFunction::Fixed,
    };

    setup_test!(
        worker
        extract(c, protocol_yield_user)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_fee = fee;
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 1000)
        .await;

    eprintln!("Sleeping 10s...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    let supply_user_balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;
    let yield_before = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .map_or(0, |r| u128::from(r.get_total()));

    c.create_supply_withdrawal_request(&supply_user, 1000).await;
    c.execute_next_supply_withdrawal_request(&supply_user, None)
        .await;

    let supply_user_balance_after = c.borrow_asset.balance_of(supply_user.id()).await;
    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;
    let yield_after: u128 = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .unwrap()
        .get_total()
        .into();

    assert_eq!(
        supply_user_balance_after,
        supply_user_balance_before + 900,
        "Fee should be applied to early withdrawal",
    );

    assert_eq!(
        yield_after,
        yield_before + 100,
        "Fee should be credited to the protocol account",
    );
}

#[rstest]
#[tokio::test]
async fn supply_withdrawal_fee_expired(#[future(awt)] worker: Worker<Sandbox>) {
    let fee = TimeBasedFee {
        fee: Fee::Flat(100.into()),
        duration: U64(1000), // 1 second
        behavior: TimeBasedFeeFunction::Fixed,
    };

    setup_test!(
        worker
        extract(c, protocol_yield_user)
        accounts(supply_user)
        config(|c| {
            c.supply_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_range = (100, None).try_into().unwrap();
            c.supply_withdrawal_fee = fee;
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 1000)
        .await;

    eprintln!("Sleeping 10s...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    let supply_user_balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
    c.accumulate_static_yield(&protocol_yield_user, None, None)
        .await;
    let yield_before = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .map_or(0, |r| u128::from(r.get_total()));

    c.create_supply_withdrawal_request(&supply_user, 1000).await;
    c.execute_next_supply_withdrawal_request(&supply_user, None)
        .await;

    let supply_user_balance_after = c.borrow_asset.balance_of(supply_user.id()).await;
    let yield_after = u128::from(
        c.get_static_yield(protocol_yield_user.id())
            .await
            .unwrap()
            .get_total(),
    );

    assert_eq!(
        supply_user_balance_after,
        supply_user_balance_before + 1000,
        "Fee should not be applied after period expires",
    );

    assert_eq!(
        yield_after, yield_before,
        "Fee should not be credited after period expires",
    );
}
