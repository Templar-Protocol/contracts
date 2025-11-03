use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;

use test_utils::*;

#[rstest]
#[tokio::test]
async fn empty_positions_are_removed(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user)
    );

    c.supply(&supply_user, 1000).await;

    assert!(c.get_supply_position(supply_user.id()).await.is_some());

    c.create_supply_withdrawal_request(&supply_user, 1000).await;
    c.execute_next_supply_withdrawal_request(&supply_user, None)
        .await;
    assert!(c.get_supply_position(supply_user.id()).await.is_none());

    tokio::join!(
        async {
            // Deposit a little bit more again.
            c.storage_deposit(&supply_user, c.storage_balance_bounds().await.min)
                .await;
            c.supply_and_harvest_until_activation(&supply_user, 1000)
                .await;
        },
        async {
            c.collateralize(&borrow_user, 2000).await;
            assert!(c.get_borrow_position(borrow_user.id()).await.is_some());

            c.withdraw_collateral(&borrow_user, 2000).await;
            assert!(c.get_borrow_position(borrow_user.id()).await.is_none());
        }
    );

    c.create_supply_withdrawal_request(&supply_user, 1000).await;
    c.execute_next_supply_withdrawal_request(&supply_user, None)
        .await;
    assert!(c.get_supply_position(supply_user.id()).await.is_none());
}
