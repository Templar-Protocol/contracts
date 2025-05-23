use near_sdk::json_types::U64;
use templar_common::borrow::{BorrowStatus, LiquidationReason};
use test_utils::*;

#[tokio::test]
async fn liquidation_after_expiration() {
    setup_test!(
        extract(c, worker)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_maximum_duration_ms = Some(U64(1000));
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 1000),
        c.collateralize(&borrow_user, 2000),
    );
    c.borrow(&borrow_user, 100).await;

    let prices = c.get_prices().await;

    let status = c
        .get_borrow_status(borrow_user.id(), prices.clone())
        .await
        .unwrap();

    assert!(status.is_healthy());

    worker.fast_forward(10).await.unwrap();

    let status = c.get_borrow_status(borrow_user.id(), prices).await.unwrap();

    assert_eq!(
        status,
        BorrowStatus::Liquidation(LiquidationReason::Expiration),
        "Borrow should be in liquidation after expiration",
    );
}
