use test_utils::*;

#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn cannot_borrow_untracked_funds() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    tokio::join!(
        async {
            c.supply_and_harvest_until_activation(&supply_user, 10_000)
                .await;
            c.borrow_asset_transfer(&supply_user, c.contract.id(), 10_000)
                .await;
        },
        c.collateralize(&borrow_user, 20_000),
    );

    c.borrow(&borrow_user, 12_000).await;
}

#[tokio::test]
async fn can_withdraw_untracked_funds() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    tokio::join!(
        async {
            c.supply_and_harvest_until_activation(&supply_user, 10_000)
                .await;
            c.borrow_asset_transfer(&supply_user, c.contract.id(), 8_000)
                .await;
        },
        c.collateralize(&borrow_user, 20_000),
    );
    c.borrow(&borrow_user, 8_000).await;

    let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
    c.create_supply_withdrawal_request(&supply_user, 10_000)
        .await;
    c.execute_next_supply_withdrawal_request(&supply_user).await;
    let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;
    assert_eq!(balance_before + 10_000, balance_after);
}
