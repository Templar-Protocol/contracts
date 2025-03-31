use near_sdk::serde_json::json;
use near_sdk_contract_tools::standard::nep145::StorageBalanceBounds;
use rstest::rstest;
use tokio::join;

use templar_common::{
    borrow::BorrowStatus, dec, interest_rate_strategy::InterestRateStrategy,
    market::HarvestYieldMode, number::Decimal,
};
use test_utils::*;

#[rstest]
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn test_happy() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        protocol_yield_user,
        insurance_yield_user,
        ..
    } = setup_everything(|c| {
        c.borrow_interest_rate_strategy =
            InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
    })
    .await;

    let configuration = c.get_configuration().await;

    assert_eq!(
        &configuration.collateral_asset.into_nep141().unwrap(),
        c.collateral_asset.id(),
    );
    assert_eq!(
        &configuration.borrow_asset.into_nep141().unwrap(),
        c.borrow_asset.id(),
    );

    assert!(configuration.borrow_mcr.near_equal(dec!("1.2")));

    let bounds = c
        .contract
        .view("storage_balance_bounds")
        .args_json(json!({}))
        .await
        .unwrap()
        .json::<StorageBalanceBounds>()
        .unwrap();

    assert!(!bounds.min.is_zero());

    let snapshots_len = c.get_snapshots_len().await;
    assert_eq!(snapshots_len, 1, "Should generate single snapshot on init");

    let snapshots = c.list_snapshots(None, None).await;
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].yield_distribution.is_zero());
    assert!(snapshots[0].deposited.is_zero());
    assert!(snapshots[0].borrowed.is_zero());

    // Step 1: Supply user sends tokens to contract to use for borrows.
    c.supply(&supply_user, 1100).await;

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(
        u128::from(supply_position.get_borrow_asset_deposit()),
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
    let balance_before = c.borrow_asset_balance_of(borrow_user.id()).await;

    // Borrowing 1000 borrow tokens with 2000 collateral tokens should be fine given equal price and MCR of 120%.
    c.borrow(&borrow_user, 1000).await;

    let balance_after = c.borrow_asset_balance_of(borrow_user.id()).await;

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

    c.repay(&borrow_user, 1100).await;

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
                c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
                    .await;
                let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
                assert_eq!(
                    u128::from(supply_position.borrow_asset_yield.get_total()),
                    80,
                );
                // Move the yield to the principal so that it can be withdrawn
                let amount_moved_to_principal = c
                    .harvest_yield(&supply_user, Some(HarvestYieldMode::Compounding))
                    .await;

                assert_eq!(
                    amount_moved_to_principal,
                    supply_position.borrow_asset_yield.get_total(),
                );

                let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
                // Withdraw all
                c.create_supply_withdrawal_request(&supply_user, 80).await;
                c.execute_next_supply_withdrawal_request(&supply_user).await;
                let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;

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

                let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
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

                c.execute_next_supply_withdrawal_request(&supply_user).await;

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

                let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;

                assert_eq!(balance_after - balance_before, 1100);
            }

            // Check that supply position is closed.
            {
                let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
                assert!(supply_position.get_borrow_asset_deposit().is_zero());
            }
        },
        // Protocol yield.
        async {
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert!(protocol_yield.collateral_asset.is_zero());
            assert_eq!(u128::from(protocol_yield.borrow_asset), 10);
            let balance_before = c.borrow_asset_balance_of(protocol_yield_user.id()).await;
            let result = c
                .withdraw_static_yield(&protocol_yield_user, None, None)
                .await;
            for receipt in result.receipt_outcomes() {
                assert!(&receipt.executor_id != c.collateral_asset.id());
            }
            assert!(result.failures().is_empty());
            let balance_after = c.borrow_asset_balance_of(protocol_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Insurance yield.
        async {
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert!(insurance_yield.collateral_asset.is_zero());
            assert_eq!(u128::from(insurance_yield.borrow_asset), 10);
            let balance_before = c.borrow_asset_balance_of(insurance_yield_user.id()).await;
            let result = c
                .withdraw_static_yield(&insurance_yield_user, None, None)
                .await;
            for receipt in result.receipt_outcomes() {
                assert!(&receipt.executor_id != c.collateral_asset.id());
            }
            assert!(result.failures().is_empty());
            let balance_after = c.borrow_asset_balance_of(insurance_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Borrower withdraws collateral.
        async {
            let balance_before = c.collateral_asset_balance_of(borrow_user.id()).await;
            c.withdraw_collateral(&borrow_user, 2000).await;
            let balance_after = c.collateral_asset_balance_of(borrow_user.id()).await;
            assert_eq!(balance_after - balance_before, 2000);
            let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
            assert!(!borrow_position.exists());
        },
    );
}
