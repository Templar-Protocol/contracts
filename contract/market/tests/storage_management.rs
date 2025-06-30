use test_utils::*;

#[tokio::test]
#[should_panic = "is not registered"]
async fn registration_is_required() {
    setup_test!(extract(c, worker) accounts(supply_user));

    let unregistered_account = worker.dev_create_account().await.unwrap();
    c.borrow_asset
        .transfer(&supply_user, unregistered_account.id(), 10_000)
        .await;

    c.supply(&unregistered_account, 1000).await;
}
