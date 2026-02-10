use near_sandbox::Sandbox;
use rstest::rstest;

use templar_common::{
    fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use test_utils::*;

#[rstest]
#[tokio::test]
async fn activates_in_next_snapshot(#[future(awt)] worker: Sandbox) {
    setup_test!(
        worker
        extract(c)
        accounts(supply_user, borrow_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
    );

    c.collateralize(&borrow_user, 2_000_000).await;
    c.supply(&supply_user, 1_000_000).await;
    let funds_activated_at_snapshot_index = c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming[0]
        .activate_at_snapshot_index;

    eprintln!("Funds activated at snapshot index: {funds_activated_at_snapshot_index}");
    let current_snapshot_index = c.get_finalized_snapshots_len().await;
    eprintln!("Current snapshot index: {current_snapshot_index}");

    assert_eq!(
        funds_activated_at_snapshot_index,
        current_snapshot_index + 1,
        "Funds should be activated in the next snapshot",
    );

    let balance_before = c.borrow_asset.balance_of(borrow_user.id()).await;
    c.borrow(&borrow_user, 1_000).await;
    let balance_after = c.borrow_asset.balance_of(borrow_user.id()).await;
    assert_eq!(
        balance_before + 1_000,
        balance_after,
        "Should be available in very next snapshot"
    );
}
