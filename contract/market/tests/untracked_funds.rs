use test_utils::*;

#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn cannot_borrow_untracked_funds() {
    setup_test!(extract(c) accounts(borrow_user, supply_user));

    c.supply(&supply_user, 10_000).await;
    c.borrow_asset
        .ft_transfer(&supply_user, c.market.contract().id(), 10_000)
        .await;
    c.collateralize(&borrow_user, 20_000).await;
    c.borrow(&borrow_user, 12_000).await;
}

#[tokio::test]
async fn can_withdraw_untracked_funds() {
    setup_test!(extract(c) accounts(borrow_user, supply_user));

    c.supply(&supply_user, 10_000).await;
    c.borrow_asset
        .ft_transfer(&supply_user, c.market.contract().id(), 8_000)
        .await;
    c.collateralize(&borrow_user, 20_000).await;
    c.borrow(&borrow_user, 8_000).await;

    let balance_before = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;
    c.execute_next_supply_withdrawal_request(&supply_user).await;
    let balance_after = c.borrow_asset.ft_balance_of(supply_user.id()).await.0;
    assert_eq!(balance_before + 10_000, balance_after);
}
