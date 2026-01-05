// This test is particularly long-running. Since tests are run in lexographical
// order, this test is named 0_... to start it running sooner.

use std::{collections::HashSet, sync::Arc, time::Duration};

use near_sdk::NearToken;
use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use test_utils::*;
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task::JoinSet,
};

const COUNT: usize = 100;

#[allow(clippy::too_many_lines)]
#[rstest]
#[tokio::test]
async fn many_accounts(#[future(awt)] worker: Worker<Sandbox>) {
    setup_test!(worker extract(c) accounts(first_supply));

    c.supply_and_harvest_until_activation(&first_supply, 100_000)
        .await;

    let mut set = JoinSet::new();
    let c = Arc::new(c);

    let (wq_send, mut wq_recv) = mpsc::channel::<oneshot::Sender<()>>(COUNT);

    set.spawn({
        let c = Arc::clone(&c);
        let first_supply = first_supply.clone();
        async move {
            while let Some(s) = wq_recv.recv().await {
                c.execute_next_supply_withdrawal_request(&first_supply, None)
                    .await;
                s.send(()).unwrap();
            }
        }
    });

    let (send, mut recv) = mpsc::channel(COUNT);

    set.spawn({
        let worker = worker.clone();
        async move {
            let mut total = 0;

            'outer: for i in 0.. {
                let account = create_prefixed_account(&format!("borrower_{i}"), &worker).await;
                for _ in 0..100 {
                    let sub = account
                        .create_subaccount(&format!("sub_{total}"))
                        .initial_balance(NearToken::from_near(9).saturating_div(10))
                        .transact()
                        .await
                        .unwrap()
                        .unwrap();

                    send.send((total, sub)).await.unwrap();
                    total += 1;
                    if total >= COUNT {
                        break 'outer;
                    }
                }
            }
        }
    });

    let suppliers = Arc::new(Mutex::new(HashSet::from([first_supply.id().clone()])));
    let borrowers = Arc::new(Mutex::new(HashSet::new()));

    while let Some((index, account)) = recv.recv().await {
        set.spawn({
            let c = Arc::clone(&c);
            let suppliers = Arc::clone(&suppliers);
            let borrowers = Arc::clone(&borrowers);
            let wq_send = wq_send.clone();

            async move {
                c.init_account(&account).await;

                if index & 1 == 0 {
                    c.supply_and_harvest_until_activation(&account, 100_000)
                        .await;
                    assert_eq!(
                        c.get_supply_position(account.id())
                            .await
                            .unwrap()
                            .get_deposit()
                            .active,
                        100_000.into(),
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    c.create_supply_withdrawal_request(&account, 100_000).await;
                    let balance_before = c.borrow_asset.balance_of(account.id()).await;

                    let (s, wq_empty) = oneshot::channel::<()>();
                    wq_send.send(s).await.unwrap();
                    wq_empty.await.unwrap();

                    let balance_after = c.borrow_asset.balance_of(account.id()).await;
                    assert_eq!(balance_before + 100_000, balance_after);
                    suppliers.lock().await.insert(account.id().clone());
                } else {
                    c.collateralize(&account, 100_000).await;
                    c.borrow(&account, 40_000).await;
                    let position = c.get_borrow_position(account.id()).await.unwrap();
                    assert_eq!(position.get_borrow_asset_principal(), 40_000.into());
                    assert!(position.get_total_borrow_asset_liability() >= 40_000.into());
                    assert_eq!(position.collateral_asset_deposit, 100_000.into());
                    c.repay(&account, None, 45_000).await;
                    let position = c.get_borrow_position(account.id()).await.unwrap();
                    assert!(position.get_borrow_asset_principal().is_zero());
                    assert!(position.get_total_borrow_asset_liability().is_zero());
                    assert_eq!(position.collateral_asset_deposit, 100_000.into());
                    borrowers.lock().await.insert(account.id().clone());
                }
            }
        });
    }

    drop(wq_send);

    set.join_all().await;

    let suppliers_expected = suppliers.lock().await;
    let borrowers_expected = borrowers.lock().await;

    let suppliers_actual = c.list_supply_positions(None, None).await;
    let borrowers_actual = c.list_borrow_positions(None, None).await;

    assert_eq!(suppliers_actual.len(), suppliers_expected.len());
    assert_eq!(borrowers_actual.len(), borrowers_expected.len());

    for id in suppliers_expected.iter() {
        assert!(suppliers_actual.contains_key(id));
    }

    for id in borrowers_expected.iter() {
        assert!(borrowers_actual.contains_key(id));
    }
}
