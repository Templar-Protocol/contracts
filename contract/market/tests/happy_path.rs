use near_workspaces::{network::Sandbox, Worker};
use rstest::rstest;
use tokio::join;

use templar_common::{
    asset::FungibleAsset, borrow::BorrowStatus, dec, interest_rate_strategy::InterestRateStrategy,
    market::HarvestYieldMode, Decimal,
};
use test_utils::*;

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn test_happy(
    #[future(awt)] worker: Worker<Sandbox>,
    #[case] borrow_mt: bool,
    #[case] collateral_mt: bool,
) {
    setup_test!(
        worker
        extract(c, protocol_yield_user, insurance_yield_user)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
            if borrow_mt {
                c.borrow_asset =
                    FungibleAsset::nep245(
                        c.borrow_asset.clone().into_nep141().unwrap(),
                        "mt_borrow".into(),
                    );
            }
            if collateral_mt {
                c.collateral_asset =
                    FungibleAsset::nep245(
                        c.collateral_asset.clone().into_nep141().unwrap(),
                        "mt_collateral".into(),
                    );
            }
        })
    );

    let configuration = c.get_configuration().await;

    if collateral_mt {
        assert_eq!(
            &configuration.collateral_asset.into_nep245().unwrap(),
            &(
                c.collateral_asset.contract().id().clone(),
                "mt_collateral".to_string()
            ),
        );
    } else {
        assert_eq!(
            &configuration.collateral_asset.into_nep141().unwrap(),
            c.collateral_asset.contract().id(),
        );
    }

    if borrow_mt {
        assert_eq!(
            &configuration.borrow_asset.into_nep245().unwrap(),
            &(
                c.borrow_asset.contract().id().clone(),
                "mt_borrow".to_string()
            ),
        );
    } else {
        assert_eq!(
            &configuration.borrow_asset.into_nep141().unwrap(),
            c.borrow_asset.contract().id(),
        );
    }

    assert!(configuration.borrow_mcr_liquidation.near_equal(dec!("1.2")));

    let bounds = c.storage_balance_bounds().await;

    assert!(!bounds.min.is_zero());

    let snapshots_len = c.get_finalized_snapshots_len().await;
    assert_eq!(snapshots_len, 1, "Should generate single snapshot on init");

    let snapshots = c.list_finalized_snapshots(None, None).await;
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].yield_distribution.is_zero());
    assert!(snapshots[0].borrow_asset_deposited_active.is_zero());
    assert!(snapshots[0].borrow_asset_borrowed.is_zero());

    // Step 1: Supply user sends tokens to contract to use for borrows.
    c.supply(&supply_user, 1100).await;

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(
        u128::from(supply_position.total_incoming()),
        1100,
        "Supply position should match amount of tokens supplied to contract",
    );

    // Wait for activation.
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_empty()
    {
        c.harvest_yield(&supply_user, None, None).await;
    }

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(
        u128::from(supply_position.get_deposit().active),
        1100,
        "Supply position should match amount of tokens supplied to contract",
    );

    // Step 2: Borrow user deposits collateral

    c.collateralize(&borrow_user, 2000).await;

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(
        u128::from(borrow_position.collateral_asset_deposit),
        2000,
        "Collateral asset deposit should be equal to the number of collateral tokens sent",
    );

    let borrow_status = c
        .get_borrow_status(borrow_user.id(), c.get_prices().await)
        .await
        .unwrap();

    assert_eq!(
        borrow_status,
        BorrowStatus::Healthy,
        "Borrow should be healthy when no assets are borrowed",
    );

    // Step 3: Withdraw some of the borrow asset
    let balance_before = c.borrow_asset.balance_of(borrow_user.id()).await;

    // Borrowing 1000 borrow tokens with 2000 collateral tokens should be fine given equal price and MCR of 120%.
    c.borrow(&borrow_user, 1000).await;

    let balance_after = c.borrow_asset.balance_of(borrow_user.id()).await;

    assert_eq!(
        balance_before + 1000,
        balance_after,
        "Borrow user should receive assets"
    );

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(u128::from(borrow_position.collateral_asset_deposit), 2000);
    assert_eq!(
        u128::from(borrow_position.get_total_borrow_asset_liability()),
        1000 + 100, // origination fee
    );

    // Step 4: Repay borrow

    c.repay(&borrow_user, None, 1100).await;

    // Ensure borrow is paid off.
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(u128::from(borrow_position.collateral_asset_deposit), 2000);
    assert_eq!(
        u128::from(borrow_position.get_total_borrow_asset_liability()),
        0,
    );

    join!(
        // Supply withdrawals.
        async {
            // Withdraw yield.
            {
                c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
                    .await;
                let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
                assert_eq!(
                    u128::from(supply_position.borrow_asset_yield.get_total()),
                    80,
                );

                let balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
                // Withdraw all
                c.create_supply_withdrawal_request(&supply_user, 80).await;
                c.execute_next_supply_withdrawal_request(&supply_user, None)
                    .await;
                let balance_after = c.borrow_asset.balance_of(supply_user.id()).await;

                assert_eq!(
                    balance_after - balance_before,
                    u128::from(supply_position.borrow_asset_yield.get_total()),
                );

                let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
                assert!(
                    supply_position.borrow_asset_yield.get_total().is_zero(),
                    "Supply position should not have yield after withdrawing all",
                );
            }

            // Withdraw supply.
            {
                // Queue should be empty at first.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await;
                assert!(
                    request_status.is_none(),
                    "Supply user should not be enqueued yet.",
                );
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert!(queue_status.depth.is_zero());
                assert_eq!(queue_status.length, 0);

                let balance_before = c.borrow_asset.balance_of(supply_user.id()).await;
                c.create_supply_withdrawal_request(&supply_user, 1100).await;

                // Queue should have 1 request now.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await
                    .expect("Should be enqueued now");
                assert_eq!(u128::from(request_status.amount), 1100);
                assert_eq!(u128::from(request_status.depth), 0);
                assert_eq!(request_status.index, 0);
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert_eq!(u128::from(queue_status.depth), 1100);
                assert_eq!(queue_status.length, 1);

                c.execute_next_supply_withdrawal_request(&supply_user, None)
                    .await;

                // Check the queue is empty again.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await;
                assert!(
                    request_status.is_none(),
                    "Supply user should not be enqueued yet.",
                );
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert!(queue_status.depth.is_zero());
                assert_eq!(queue_status.length, 0);

                let balance_after = c.borrow_asset.balance_of(supply_user.id()).await;

                assert_eq!(balance_after - balance_before, 1100);
            }

            // Check that supply position is closed.
            {
                let supply_position = c.get_supply_position(supply_user.id()).await;
                assert!(supply_position.is_none());
            }
        },
        // Protocol yield.
        async {
            c.accumulate_static_yield(&protocol_yield_user, None, None)
                .await;
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(protocol_yield.get_total()), 10);
            let balance_before = c.borrow_asset.balance_of(protocol_yield_user.id()).await;
            let result = c.withdraw_static_yield(&protocol_yield_user, None).await;
            for receipt in result.receipt_outcomes() {
                assert!(&receipt.executor_id != c.collateral_asset.contract().id());
            }
            assert!(result.failures().is_empty());
            let balance_after = c.borrow_asset.balance_of(protocol_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Insurance yield.
        async {
            c.accumulate_static_yield(&insurance_yield_user, None, None)
                .await;
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert_eq!(u128::from(insurance_yield.get_total()), 10);
            let balance_before = c.borrow_asset.balance_of(insurance_yield_user.id()).await;
            let result = c.withdraw_static_yield(&insurance_yield_user, None).await;
            for receipt in result.receipt_outcomes() {
                assert!(&receipt.executor_id != c.collateral_asset.contract().id());
            }
            assert!(result.failures().is_empty());
            let balance_after = c.borrow_asset.balance_of(insurance_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Borrower withdraws collateral.
        async {
            let balance_before = c.collateral_asset.balance_of(borrow_user.id()).await;
            c.withdraw_collateral(&borrow_user, 2000).await;
            let balance_after = c.collateral_asset.balance_of(borrow_user.id()).await;
            assert_eq!(balance_after - balance_before, 2000);
            let borrow_position = c.get_borrow_position(borrow_user.id()).await;
            assert!(borrow_position.is_none());
        },
    );
}
