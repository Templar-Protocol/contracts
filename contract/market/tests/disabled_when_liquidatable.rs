use test_utils::*;

#[tokio::test]
async fn collateralization() {
    setup_test!(extract(c) accounts(borrow_user, supply_user));

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
        collateral_before, collateral_after,
        "Must disallow adding collateral if position is in liquidation",
    );
}

#[tokio::test]
async fn repayment() {
    setup_test!(extract(c) accounts(borrow_user, supply_user));

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
