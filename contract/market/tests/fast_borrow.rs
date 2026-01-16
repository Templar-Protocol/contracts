use near_sandbox::Sandbox;
use rstest::rstest;

use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use test_utils::*;

#[rstest]
#[tokio::test]
async fn fast_borrow_is_not_free(#[future(awt)] worker: Sandbox) {
    setup_test!(
        worker
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(60 * 1000); // 1 minute
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 2_000_000)
        .await;

    let snapshot_len_before = c.get_finalized_snapshots_len().await;
    c.collateralize(&borrow_user, 2_000_000).await;
    c.borrow(&borrow_user, 1_000_000).await;

    // Repay exact amount that was borrowed
    c.repay(&borrow_user, None, 1_000_000).await;

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    eprintln!("{borrow_position:#?}");

    assert!(
        !borrow_position.get_total_borrow_asset_liability().is_zero(),
        "Borrow position should not have zero liability",
    );

    let snapshot_len_after = c.get_finalized_snapshots_len().await;

    assert_eq!(
        snapshot_len_before, snapshot_len_after,
        "Test should run within a single snapshot",
    );
}
