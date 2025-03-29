use test_utils::*;

#[tokio::test]
#[should_panic = "is not registered"]
async fn registration_is_required() {
    let SetupEverything { c, supply_user, .. } = setup_everything(|_| {}).await;

    let unregistered_account = c.worker.dev_create_account().await.unwrap();
    c.borrow_asset_transfer(&supply_user, unregistered_account.id(), 10000)
        .await;

    c.supply(&unregistered_account, 1000).await;
}
