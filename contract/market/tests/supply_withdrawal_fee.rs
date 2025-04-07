use std::time::Duration;

use near_sdk::json_types::U64;
use templar_common::fee::{Fee, TimeBasedFee, TimeBasedFeeFunction};
use test_utils::*;

#[tokio::test]
async fn supply_withdrawal_fee_flat() {
    let fee = TimeBasedFee {
        fee: Fee::Flat(100.into()),
        duration: U64(1000 * 60 * 60 * 24 * 30),
        behavior: TimeBasedFeeFunction::Fixed,
    };

    let SetupEverything {
        c,
        supply_user,
        protocol_yield_user,
        ..
    } = setup_everything(|c| {
        c.supply_withdrawal_fee = fee;
    })
    .await;

    c.supply(&supply_user, 1000).await;

    eprintln!("Sleeping 10s...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    let supply_user_balance_before = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    let yield_before = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .map_or(0, |r| u128::from(r.borrow_asset));

    c.create_supply_withdrawal_request(&supply_user, 1000.into())
        .await;
    c.execute_next_supply_withdrawal_request(&supply_user).await;

    let supply_user_balance_after = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    let yield_after = u128::from(
        c.get_static_yield(protocol_yield_user.id())
            .await
            .unwrap()
            .borrow_asset,
    );

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

#[tokio::test]
async fn supply_withdrawal_fee_expired() {
    let fee = TimeBasedFee {
        fee: Fee::Flat(100.into()),
        duration: U64(1000), // 1 second
        behavior: TimeBasedFeeFunction::Fixed,
    };

    let SetupEverything {
        c,
        supply_user,
        protocol_yield_user,
        ..
    } = setup_everything(|c| {
        c.supply_withdrawal_fee = fee;
    })
    .await;

    c.supply(&supply_user, 1000).await;

    eprintln!("Sleeping 10s...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    let supply_user_balance_before = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    let yield_before = c
        .get_static_yield(protocol_yield_user.id())
        .await
        .map_or(0, |r| u128::from(r.borrow_asset));

    c.create_supply_withdrawal_request(&supply_user, 1000.into())
        .await;
    c.execute_next_supply_withdrawal_request(&supply_user).await;

    let supply_user_balance_after = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    let yield_after = u128::from(
        c.get_static_yield(protocol_yield_user.id())
            .await
            .unwrap()
            .borrow_asset,
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
