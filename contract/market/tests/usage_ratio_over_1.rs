use test_utils::*;

#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn usage_ratio_over_1() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 10_000).await;
    c.borrow_asset_transfer(&supply_user, c.contract.id(), 10_000)
        .await;
    c.collateralize(&borrow_user, 20_000).await;
    c.borrow(&borrow_user, 12_000).await;
}
