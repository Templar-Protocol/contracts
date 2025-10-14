use near_sdk::{serde_json::json, NearToken};
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use tokio::task::JoinSet;

use test_utils::*;

#[rstest]
#[tokio::test]
async fn collateral_withdrawal(#[future(awt)] worker: Worker<Sandbox>) {
    let amounts = [10u128; 30];

    setup_test!(worker extract(c) accounts(borrow_user));

    let total = amounts.iter().copied().sum::<u128>();

    c.collateralize(&borrow_user, total).await;

    let mut set = JoinSet::new();

    for amount in amounts {
        let borrow_user = borrow_user.clone();
        let c = c.clone();
        set.spawn(async move {
            let r = c.withdraw_collateral(&borrow_user, amount).await;
            let succeeded = r.outcomes().iter().all(|o| o.is_success());
            if succeeded {
                amount
            } else {
                0
            }
        });
    }

    let (successful_withdrawals, ()) = tokio::join!(set.join_all(), async {
        borrow_user
            .call(
                c.collateral_asset.contract().id(),
                "patch_storage_unregister",
            )
            .args_json(json!({"force": true}))
            .deposit(NearToken::from_yoctonear(1))
            .transact()
            .await
            .unwrap()
            .into_result()
            .unwrap();
    });

    let had_failure = successful_withdrawals.iter().any(|amount| *amount == 0);
    assert!(
        had_failure,
        "At least one withdrawal should fail due to storage unregistration"
    );

    let withdrawn = successful_withdrawals.iter().sum::<u128>();

    let collateral_deposit: u128 = c
        .get_borrow_position(borrow_user.id())
        .await
        .unwrap()
        .collateral_asset_deposit
        .into();

    assert_eq!(withdrawn + collateral_deposit, total);
}
