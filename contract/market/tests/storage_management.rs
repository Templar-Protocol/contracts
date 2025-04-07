use test_utils::*;

#[tokio::test]
#[should_panic = "is not registered"]
async fn registration_is_required() {
    let SetupEverything {
        worker,
        c,
        supply_user,
        ..
    } = setup_everything(|_| {}).await;

    let unregistered_account = worker.dev_create_account().await.unwrap();
    c.borrow_asset
        .ft_transfer(&supply_user, unregistered_account.id(), 10_000.into())
        .await;

    c.supply(&unregistered_account, 1000).await;
}
