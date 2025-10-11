use templar_common::{
    dec,
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::{HarvestYieldMode, YieldWeights},
    time_chunk::TimeChunkConfiguration,
    YEAR_PER_MS,
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
            c.time_chunk_configuration = TimeChunkConfiguration::new(8 * 1000);
            c.yield_weights = YieldWeights::new_with_supply_weight(1);
        })
    );

    eprintln!("First deposit");
    c.supply(&supply_user, 1_000_000).await;
    eprintln!(
        "Funds get activated at: {}",
        c.get_supply_position(supply_user.id())
            .await
            .unwrap()
            .get_deposit()
            .incoming[0]
            .activate_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_empty()
    {
        tokio::join!(
            async {
                eprintln!(
                    "Current snapshot: {}",
                    c.get_finalized_snapshots_len().await,
                );
            },
            async {
                c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
                    .await;
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    eprintln!("First activation: {snapshot_supply_start} -> {snapshot_supply_end}");
    assert_eq!(
        snapshot_supply_start + 1,
        snapshot_supply_end,
        "Funds activate in one snapshot",
    );

    eprintln!("Second deposit");
    c.supply(&supply_user, 1_000_000).await;
    eprintln!(
        "Funds get activated at: {}",
        c.get_supply_position(supply_user.id())
            .await
            .unwrap()
            .get_deposit()
            .incoming[0]
            .activate_at_snapshot_index
    );
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_empty()
    {
        tokio::join!(
            async {
                eprintln!(
                    "Current snapshot: {}",
                    c.get_finalized_snapshots_len().await,
                );
            },
            async {
                c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
                    .await;
            },
        );
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    eprintln!("Second activation: {snapshot_supply_start} -> {snapshot_supply_end}");
    assert_eq!(
        snapshot_supply_start + 1,
        snapshot_supply_end,
        "Funds activate in one snapshot",
    );
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
            c.time_chunk_configuration = TimeChunkConfiguration::new(12 * 1000);
            c.yield_weights = YieldWeights::new_with_supply_weight(1);
        })
    );

    eprintln!("Creating first supply position");
    c.supply(&supply_user, 100_000_000).await;
    let snapshot_supply_start = c.get_finalized_snapshots_len().await;

    // Wait for activation of funds
    while !c
        .get_supply_position(supply_user.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming
        .is_empty()
    {
        c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
            .await;
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    eprintln!("Activation: {snapshot_supply_start} -> {snapshot_supply_end}");

    c.collateralize(&borrow_user, 100_000_000).await;
    c.borrow(&borrow_user, 10_000_000).await;

    let borrow_started_at_snapshot_index = c.get_finalized_snapshots_len().await;

    c.supply(&supply_user_2, 100_000_000).await;
    let funds_activate_at = c
        .get_supply_position(supply_user_2.id())
        .await
        .unwrap()
        .get_deposit()
        .incoming[0]
        .activate_at_snapshot_index;

    let snapshot_supply_start = c.get_finalized_snapshots_len().await;
    while c.get_finalized_snapshots_len().await <= funds_activate_at {
        // Interest rate is high enough that this all goes to interest.
        c.repay(&borrow_user, 10_000).await;
        c.harvest_yield(&supply_user, None, Some(HarvestYieldMode::Default))
            .await;
        c.harvest_yield(&supply_user_2, None, Some(HarvestYieldMode::Default))
            .await;
    }
    let snapshot_supply_end = c.get_finalized_snapshots_len().await;
    eprintln!("Activation: {snapshot_supply_start} -> {snapshot_supply_end}");

    let (position_1_end, position_2_end) = tokio::join!(
        async { c.get_supply_position(supply_user.id()).await.unwrap() },
        async { c.get_supply_position(supply_user_2.id()).await.unwrap() },
    );

    eprintln!("Position 1 end: {position_1_end:#?}");
    eprintln!("Position 2 end: {position_2_end:#?}");

    let amount_1_end = position_1_end.borrow_asset_yield.get_total();
    let amount_2_end = position_2_end.borrow_asset_yield.get_total();

    let snapshots = c.list_finalized_snapshots(None, None).await;

    for (i, snapshot) in snapshots.iter().enumerate() {
        eprintln!("Snapshot {i}:");
        eprintln!("{snapshot:#?}");
    }

    eprintln!("Current snapshot:");
    eprintln!("{:#?}", c.get_current_snapshot().await);

    let interest_rate = dec!("10000");

    let borrow_started_at = &snapshots[borrow_started_at_snapshot_index as usize - 1]
        .end_timestamp_ms
        .0;
    let first_supply_only_active = &snapshots[snapshots.len() - 2];
    let both_supply_active = &snapshots[snapshots.len() - 1];

    let first_only_duration_ms = first_supply_only_active.end_timestamp_ms.0 - borrow_started_at;
    let expected_first_only_interest_amount = 10_000_000_u128
        * (interest_rate * first_only_duration_ms * YEAR_PER_MS
            + c.configuration.single_snapshot_maximum_interest());

    let both_active_duration_ms =
        both_supply_active.end_timestamp_ms.0 - first_supply_only_active.end_timestamp_ms.0;
    let expected_both_active_interest_amount =
        10_000_000_u128 * interest_rate * both_active_duration_ms * YEAR_PER_MS / 2_u128;

    eprintln!("Amount 1 end: {amount_1_end}");
    eprintln!("Amount 2 end: {amount_2_end}");
    assert!(
        u128::from(amount_1_end).abs_diff(
            (expected_first_only_interest_amount + expected_both_active_interest_amount)
                .to_u128_floor()
                .unwrap(),
        ) <= 1
    );
    assert!(
        u128::from(amount_2_end).abs_diff(
            expected_both_active_interest_amount
                .to_u128_floor()
                .unwrap(),
        ) <= 1
    );
    assert_eq!(
        position_1_end.borrow_asset_yield.pending_estimate,
        position_2_end.borrow_asset_yield.pending_estimate,
    );
}
