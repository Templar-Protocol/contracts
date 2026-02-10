use near_sandbox::Sandbox;
use rstest::rstest;
use test_utils::*;

#[rstest]
#[tokio::test]
#[should_panic = "is not registered"]
async fn registration_is_required(#[future(awt)] worker: Sandbox) {
    setup_test!(worker extract(c) accounts(supply_user));
    accounts!(worker, unregistered_account);

    c.borrow_asset
        .transfer(&supply_user, unregistered_account.id(), 10_000)
        .await;

    c.supply(&unregistered_account, 1000).await;
}
