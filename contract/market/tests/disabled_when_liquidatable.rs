use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use test_utils::*;

#[rstest]
#[tokio::test]
async fn disable_collateralize_if_still_liquidatable(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 2_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    c.borrow(&borrow_user, 1_000_000).await;
    c.set_collateral_asset_price(0.5).await;
    let collateral_before = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .collateral_asset_deposit;
    c.collateralize(&borrow_user, 2_000).await;
    let collateral_after = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .collateral_asset_deposit;

    assert_eq!(
        collateral_before, collateral_after,
        "Must disallow adding collateral if position would still be in liquidation",
    );
}

#[rstest]
#[tokio::test]
async fn allow_sufficient_collateralization_during_liquidation(
    #[future(awt)] worker: Worker<Sandbox>,
) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 2_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    c.borrow(&borrow_user, 1_000_000).await;
    c.set_collateral_asset_price(0.5).await;
    let collateral_before = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .collateral_asset_deposit;
    c.collateralize(&borrow_user, 2_000_000).await;
    let collateral_after = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .collateral_asset_deposit;

    assert_eq!(
        collateral_before + 2_000_000,
        collateral_after,
        "Collateralization should be allowed if it brings the position out of liquidation",
    );
}

#[rstest]
#[tokio::test]
async fn repayment(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(borrow_user, supply_user));

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 2_000_000),
        c.collateralize(&borrow_user, 2_000_000),
    );

    c.borrow(&borrow_user, 1_000_000).await;
    c.set_collateral_asset_price(0.5).await;

    let liability_before = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .get_total_borrow_asset_liability();
    c.repay(&borrow_user, 1_050_000).await;
    let liability_after = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .get_total_borrow_asset_liability();

    assert!(
        liability_after >= liability_before,
        "Must disallow repayment if position is in liquidation",
    );
}
