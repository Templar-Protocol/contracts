use partial::check;
use std::time::Duration;
use templar_common::{
    dec, fee::Fee, interest_rate_strategy::InterestRateStrategy, time_chunk::TimeChunkConfiguration,
};
use test_utils::*;

#[tokio::test]
async fn snapshot_captures_borrow_and_collateral_state() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500); // 0.5 seconds
        })
    );

    // Setup liquidity
    c.supply_and_harvest_until_activation(&supply_user, 2_000_000)
        .await;

    let initial_snapshots_len = c.get_finalized_snapshots_len().await;

    // Perform operations within the same time chunk
    c.collateralize(&borrow_user, 1_000_000).await;
    c.borrow(&borrow_user, 500_000).await;

    // Wait for snapshot to finalize
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Trigger something to ensure snapshot finalization
    c.collateralize(&borrow_user, 1).await;
    // Snapshot updating occurs before collateral deposit is recorded, so do
    // it 2x so we can see 1 (from the preceding call) in the current snapshot.
    c.collateralize(&borrow_user, 1).await;

    let final_snapshots_len = c.get_finalized_snapshots_len().await;

    assert!(
        final_snapshots_len > initial_snapshots_len,
        "Should have created a new finalized snapshot"
    );

    // Get the latest snapshot
    let snapshots = c.list_finalized_snapshots(None, None).await;

    for (i, snapshot) in snapshots.iter().enumerate() {
        eprintln!("{i}: {snapshot:#?}");
    }

    check(
        states!(
            { active += 2_000_000 },
            { collateral += 1_000_000 },
            { borrowed += 500_000 },
            { collateral += 1 },
        ),
        snapshots,
    );
}

#[tokio::test]
async fn multiple_snapshots_show_progression() {
    setup_test!(
        extract(c)
        accounts(user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1000);
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 3_000_000)
        .await;

    // First period: collateralize
    c.collateralize(&user, 1_000_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Second period: borrow
    c.borrow(&user, 400_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Third period: more borrowing
    c.borrow(&user, 200_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Finalize last snapshot
    c.apply_interest(&user, None, None).await;

    // Get the snapshots
    let snapshots = c.list_finalized_snapshots(None, None).await;

    check(
        states!(
            { active = 3_000_000 },
            { collateral += 1_000_000 },
            { borrowed += 400_000 },
            { borrowed += 200_000 },
        ),
        snapshots,
    );
}

#[tokio::test]
async fn snapshot_reflects_repayment_changes() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy = InterestRateStrategy::zero();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 2_000_000)
        .await;
    c.collateralize(&borrow_user, 1_000_000).await;
    c.borrow(&borrow_user, 500_000).await;

    // Wait and trigger first snapshot (with borrowed amount)
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    let snapshots_after_borrow = c.get_finalized_snapshots_len().await;

    // Repay half
    c.repay(&borrow_user, 250_000).await;

    // Wait and trigger second snapshot (after partial repayment)
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    let snapshots_after_repay = c.get_finalized_snapshots_len().await;

    assert!(
        snapshots_after_repay > snapshots_after_borrow,
        "Should have created snapshot after repayment"
    );

    // Compare the two snapshots
    let all_snapshots = c.list_finalized_snapshots(None, None).await;
    let borrow_snapshot = &all_snapshots[snapshots_after_borrow as usize - 1];
    let repay_snapshot = &all_snapshots[snapshots_after_repay as usize - 1];

    let amount_after_borrow = u128::from(borrow_snapshot.borrow_asset_borrowed);
    let amount_after_repay = u128::from(repay_snapshot.borrow_asset_borrowed);

    eprintln!("After borrow: borrowed={amount_after_borrow}");
    eprintln!("After repay: borrowed={amount_after_repay}");

    assert_eq!(
        amount_after_borrow,
        amount_after_repay * 2,
        "Snapshots should reflect different borrowed states",
    );
}

#[tokio::test]
async fn snapshot_handles_zero_operations() {
    setup_test!(
        extract(c)
        accounts(supply_user)
        config(|c| {
            c.time_chunk_configuration = TimeChunkConfiguration::new(500); // 0.5 seconds
        })
    );

    // Setup initial state
    c.supply_and_harvest_until_activation(&supply_user, 1_000_000)
        .await;

    let initial_snapshots_len = c.get_finalized_snapshots_len().await;

    // Wait for time chunk to expire with no operations
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Trigger snapshot with minimal operation
    c.supply_and_harvest_until_activation(&supply_user, 1).await;

    let final_snapshots_len = c.get_finalized_snapshots_len().await;

    eprintln!("Snapshots before: {initial_snapshots_len}, after: {final_snapshots_len}");

    // Verify behavior when no meaningful operations occur
    assert!(final_snapshots_len > initial_snapshots_len);

    // Finalize snapshot
    c.harvest_yield(&supply_user, None, None).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;

    check(states!({ active = 1_000_000 }, { active += 1 }), snapshots);
}

#[tokio::test]
async fn snapshot_with_full_repayment() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
    );

    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user, 2_000_000),
        async {
            c.collateralize(&borrow_user, 1_000_000).await;
            c.borrow(&borrow_user, 500_000).await;
        },
    );

    // Create snapshot with borrowed amount
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    let total_liability = u128::from(borrow_position.get_total_borrow_asset_liability());

    eprintln!("Total liability before repayment: {total_liability:?}");

    // Repay everything (including any accrued interest)
    c.repay(&borrow_user, total_liability).await;

    // Create snapshot after full repayment
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;
    let final_snapshot = &snapshots[snapshots.len() - 1];

    eprintln!(
        "After full repayment: borrowed={:?}",
        final_snapshot.borrow_asset_borrowed,
    );

    let final_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    eprintln!(
        "Final position liability: {:?}",
        final_position.get_total_borrow_asset_liability()
    );

    eprintln!("Final snapshot:");
    eprintln!("{final_snapshot:#?}");

    // Verify snapshot reflects full repayment
    assert!(
        final_snapshot.borrow_asset_borrowed <= 1000.into(), // Allow for small rounding
        "Snapshot should show minimal borrowed amount after full repayment"
    );
}

#[tokio::test]
async fn snapshot_field_validation() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("2000"), dec!("3000")).unwrap(); // Higher rates for testing
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(500);
        })
    );

    // Step 1: Supply (affects borrow_asset_deposited fields)
    c.supply_and_harvest_until_activation(&supply_user, 1_500_000)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    // Step 2: Collateralize (affects collateral_asset_deposited)
    c.collateralize(&borrow_user, 800_000).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    // Step 3: Borrow (affects borrow_asset_borrowed, interest_rate)
    c.borrow(&borrow_user, 400_000).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.collateralize(&borrow_user, 1).await;

    // Step 4: Let interest accrue
    tokio::time::sleep(Duration::from_secs(2)).await;
    c.collateralize(&borrow_user, 1).await;

    // Finalize last snapshot
    c.collateralize(&borrow_user, 1).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;

    check(
        states!(
            { active = 1_500_000 },
            { collateral += 1 },
            { collateral += 800_000 },
            { collateral += 1 },
            { borrowed += 400_000 },
            { collateral += 1 },
            { collateral += 1 },
        ),
        &snapshots,
    );

    let mut last_end_timestamp = snapshots[0].end_timestamp_ms.0;
    for (i, snapshot) in snapshots.iter().enumerate() {
        assert!(
            snapshot.end_timestamp_ms.0 >= last_end_timestamp,
            "Timestamps did not increase at {i}",
        );
        last_end_timestamp = snapshot.end_timestamp_ms.0;
    }

    let last = &snapshots[snapshots.len() - 1];

    // Interest rate should reflect utilization
    assert!(
        !last.interest_rate.is_zero(),
        "Interest rate should be positive with borrowing activity",
    );
}

#[tokio::test]
async fn many_users_different_snapshots() {
    setup_test!(
        extract(c)
        accounts(user1, user2, user3, user4, user5, supply_user1, supply_user2)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(1);
        })
    );

    // Multiple suppliers
    c.supply_and_harvest_until_activation(&supply_user1, 2_000_000)
        .await;
    c.supply_and_harvest_until_activation(&supply_user2, 1_500_000)
        .await;

    // All collateral operations
    c.collateralize(&user1, 400_000).await;
    c.collateralize(&user2, 350_000).await;
    c.collateralize(&user3, 300_000).await;
    c.collateralize(&user4, 250_000).await;
    c.collateralize(&user5, 200_000).await;

    // All borrow operations
    c.borrow(&user1, 150_000).await;
    c.borrow(&user2, 120_000).await;
    c.borrow(&user3, 100_000).await;
    c.borrow(&user4, 80_000).await;
    c.borrow(&user5, 60_000).await;

    // Wait and trigger snapshot
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.harvest_yield(&supply_user1, None, None).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;

    check(
        states!(
            { active += 2_000_000 },
            { active += 1_500_000 },
            { collateral += 400_000 },
            { collateral += 350_000 },
            { collateral += 300_000 },
            { collateral += 250_000 },
            { collateral += 200_000 },
            { borrowed += 150_000 },
            { borrowed += 120_000 },
            { borrowed += 100_000 },
            { borrowed += 80_000 },
            { borrowed += 60_000 },
        ),
        snapshots,
    );
}

#[tokio::test]
async fn many_users_same_snapshot() {
    let w = near_workspaces::sandbox().await.unwrap();
    eprintln!("Fast-forwarding...");
    // Need to fast forward right away because otherwise the contract will panic because it can't construct a previous time chunk
    w.fast_forward(100).await.unwrap();
    setup_test_w!(
        w
        extract(c)
        accounts(user1, user2, user3, user4, user5, supply_user1, supply_user2)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::new(10_000);
        })
    );

    // Multiple suppliers
    tokio::join!(
        c.supply_and_harvest_until_activation(&supply_user1, 2_000_000),
        c.supply_and_harvest_until_activation(&supply_user2, 1_500_000),
    );

    eprintln!("Fast-forwarding...");
    w.fast_forward(100).await.unwrap();

    eprintln!("Collateral operations");
    tokio::join!(
        c.collateralize(&user1, 400_000),
        c.collateralize(&user2, 350_000),
        c.collateralize(&user3, 300_000),
        c.collateralize(&user4, 250_000),
        c.collateralize(&user5, 200_000),
    );

    eprintln!("Fast-forwarding...");
    w.fast_forward(100).await.unwrap();

    eprintln!("Borrow operations");
    tokio::join!(
        c.borrow(&user1, 150_000),
        c.borrow(&user2, 120_000),
        c.borrow(&user3, 100_000),
        c.borrow(&user4, 80_000),
        c.borrow(&user5, 60_000),
    );

    eprintln!("Fast-forwarding...");
    w.fast_forward(100).await.unwrap();

    eprintln!("Trigger snapshot");
    c.harvest_yield(&supply_user1, None, None).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;

    check(
        states!(
            { active += 2_000_000 + 1_500_000 },
            { collateral += 400_000 + 350_000 + 300_000 + 250_000 + 200_000 },
            { borrowed += 150_000 + 120_000 + 100_000 + 80_000 + 60_000 },
        ),
        snapshots,
    );
}

#[tokio::test]
async fn incoming() {
    setup_test!(
        extract(c)
        accounts(borrow_user, supply_user)
    );

    c.supply(&supply_user, 2_000_000).await;

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    let incoming_activates_at =
        supply_position.get_deposit().incoming[0].activate_at_snapshot_index;

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
    // Finalize snapshot where funds were activated
    c.harvest_yield(&supply_user, None, None).await;
    // Two more to ensure we have snapshots afterwards
    c.harvest_yield(&supply_user, None, None).await;
    c.harvest_yield(&supply_user, None, None).await;

    let snapshots = c.list_finalized_snapshots(None, None).await;

    for (i, snapshot) in snapshots.iter().enumerate() {
        eprintln!("{i}: {snapshot:#?}");
    }

    eprintln!("Should activate at: {incoming_activates_at}");

    let snapshot_before_before = &snapshots[incoming_activates_at as usize - 2];
    let snapshot_before = &snapshots[incoming_activates_at as usize - 1];
    let snapshot_at = &snapshots[incoming_activates_at as usize];
    let snapshot_after = &snapshots[incoming_activates_at as usize + 1];
    let snapshot_after_after = &snapshots[incoming_activates_at as usize + 2];

    assert!(snapshot_before_before
        .borrow_asset_deposited_active
        .is_zero());
    assert!(snapshot_before.borrow_asset_deposited_active.is_zero());
    assert_eq!(snapshot_at.borrow_asset_deposited_active, 2_000_000.into());
    assert_eq!(
        snapshot_after.borrow_asset_deposited_active,
        2_000_000.into(),
    );
    assert_eq!(
        snapshot_after_after.borrow_asset_deposited_active,
        2_000_000.into(),
    );
}
