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
            .get_deposit()
            .activate_incoming_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
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
            .get_deposit()
            .activate_incoming_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
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
        .get_deposit()
        .incoming
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
    let mut earned_in_first_snapshot = 0u128;
    while !c
        .get_supply_position(supply_user_2.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_zero()
    {
        tokio::join!(
            c.harvest_yield(&supply_user, Some(HarvestYieldMode::Default)),
            c.harvest_yield(&supply_user_2, Some(HarvestYieldMode::Default)),
            async {
                c.repay(&borrow_user, 10_000).await;
                c.borrow(&borrow_user, 10_000).await;
            },
            async {
                let position = c.get_supply_position(supply_user.id()).await.unwrap();
                let total = position.borrow_asset_yield.get_total();
                let pending = position.borrow_asset_yield.pending_estimate;
                eprintln!("Older position total: {total}");
                eprintln!("Older position pending: {pending}");

                if !total.is_zero() && earned_in_first_snapshot == 0 {
                    earned_in_first_snapshot = total.into();
                }
            },
            async {
                let position = c.get_supply_position(supply_user_2.id()).await.unwrap();
                let total = position.borrow_asset_yield.get_total();
                let pending = position.borrow_asset_yield.pending_estimate;
                eprintln!("Newer position total: {total}");
                eprintln!("Newer position pending: {pending}");
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    println!("Activation: {snapshot_supply_start} -> {snapshot_supply_end}");

    let (position_1_end, position_2_end) = tokio::join!(
        async { c.get_supply_position(supply_user.id()).await.unwrap() },
        async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
    );

    eprintln!("Position 1 end: {position_1_end:#?}");
    eprintln!("Position 2 end: {position_2_end:#?}");

    let amount_1_end = position_1_end.borrow_asset_yield.get_total();
    let amount_2_end = position_2_end.borrow_asset_yield.get_total();

    eprintln!("Amount earned in first snapshot: {earned_in_first_snapshot}");
    eprintln!("Amount 1 end: {amount_1_end}");
    eprintln!("Amount 2 end: {amount_2_end}");
    // assert!(u128::from(amount_2_end) * 2 <= u128::from(amount_1_end));
    assert_eq!(
        u128::from(amount_1_end),
        u128::from(amount_2_end) + earned_in_first_snapshot,
    );
    assert_eq!(
        position_1_end.borrow_asset_yield.pending_estimate,
        position_2_end.borrow_asset_yield.pending_estimate,
    );
}
