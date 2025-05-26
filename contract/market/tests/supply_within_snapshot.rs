use templar_common::{
    dec,
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::{HarvestYieldMode, YieldWeights},
    time_chunk::TimeChunkConfiguration,
};
use test_utils::*;

#[tokio::test]
async fn funds_activation() {
    setup_test!(
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("10000"), dec!("10000")).unwrap();
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: (8 * 1000).into(),
            };
            c.yield_weights = YieldWeights::new_with_supply_weight(1);
        })
    );

    println!("First deposit");
    c.supply(&supply_user, 1_000_000).await;
    println!(
        "Funds get activated at: {}",
        c.get_supply_position(supply_user.id())
            .await
            .unwrap()
            .get_inactive_deposit()
            .activate_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_inactive_deposit()
        .amount
        .is_zero()
    {
        tokio::join!(
            async {
                println!(
                    "Current snapshot: {}",
                    c.get_finalized_snapshots_len().await,
                );
            },
            async {
                c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
                    .await;
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    println!("First activation: {snapshot_supply_start} -> {snapshot_supply_end}");
    // Funds activate in one snapshot, but the funds are not moved until the snapshot is finalized, so it appears to take two snapshots (but yield will be earned for the second one).
    assert_eq!(snapshot_supply_start + 2, snapshot_supply_end);

    println!("Second deposit");
    c.supply(&supply_user, 1_000_000).await;
    println!(
        "Funds get activated at: {}",
        c.get_supply_position(supply_user.id())
            .await
            .unwrap()
            .get_inactive_deposit()
            .activate_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_inactive_deposit()
        .amount
        .is_zero()
    {
        tokio::join!(
            async {
                println!(
                    "Current snapshot: {}",
                    c.get_finalized_snapshots_len().await,
                );
            },
            async {
                c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
                    .await;
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    println!("Second activation: {snapshot_supply_start} -> {snapshot_supply_end}");
    assert_eq!(snapshot_supply_start + 2, snapshot_supply_end);
}

#[tokio::test]
async fn partial_snapshot_no_earnings() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user, supply_user_2)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("10000"), dec!("10000")).unwrap();
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: (12 * 1000).into(),
            };
            c.yield_weights = YieldWeights::new_with_supply_weight(1);
        })
    );

    println!("Creating first supply position");
    c.supply(&supply_user, 100_000_000).await;
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_inactive_deposit()
        .amount
        .is_zero()
    {
        c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default))
            .await;
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    println!("Activation: {snapshot_supply_start} -> {snapshot_supply_end}");

    c.collateralize(&borrow_user, 100_000_000).await;
    c.borrow(&borrow_user, 10_000_000).await;

    c.supply(&supply_user_2, 100_000_000).await;

    let snapshot_supply_start = c.get_finalized_snapshots_len().await;
    while !c
        .get_supply_position(supply_user_2.id())
        .await
        .unwrap()
        .get_inactive_deposit()
        .amount
        .is_zero()
    {
        tokio::join!(
            c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default)),
            c.harvest_yield(&supply_user_2, Some(HarvestYieldMode::Default)),
            async {
                c.repay(&borrow_user, 10_000).await;
                c.borrow(&borrow_user, 10_000).await;
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    println!("Activation: {snapshot_supply_start} -> {snapshot_supply_end}");

    let (amount_1_end, amount_2_end) = tokio::join!(
        async { c.get_supply_position(supply_user.id()).await.unwrap() },
        async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
    );

    assert!(
        u128::from(amount_2_end.borrow_asset_yield.get_total()) * 2
            <= u128::from(amount_1_end.borrow_asset_yield.get_total())
    );
    assert_eq!(
        amount_1_end.borrow_asset_yield.pending_estimate,
        amount_2_end.borrow_asset_yield.pending_estimate,
    );
}
