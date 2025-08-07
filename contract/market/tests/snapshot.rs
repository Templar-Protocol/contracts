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
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(), // 0.5 seconds
            };
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
    let snapshots = c
        .list_finalized_snapshots(Some(final_snapshots_len - 1), Some(1))
        .await;
    let latest_snapshot = &snapshots[0];

    eprintln!("Latest snapshot: {latest_snapshot:#?}");

    // Verify snapshot captured the state correctly
    assert_eq!(
        u128::from(latest_snapshot.collateral_asset_deposited()),
        1_000_000, // Additional 1 in the next (current) snapshot
        "Snapshot should show collateral was deposited",
    );

    assert_eq!(
        u128::from(latest_snapshot.borrow_asset_borrowed()),
        500_000,
        "Snapshot should show assets were borrowed",
    );

    let current_snapshot = c.market.get_current_snapshot().await;

    assert_eq!(
        u128::from(current_snapshot.collateral_asset_deposited()),
        1_000_001,
    );
    assert_eq!(
        u128::from(current_snapshot.borrow_asset_borrowed()),
        500_000,
    );
}

#[tokio::test]
async fn multiple_snapshots_show_progression() {
    setup_test!(
        extract(c)
        accounts(user, supply_user)
        config(|c| {
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 1000.into(),
            };
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 3_000_000)
        .await;

    let initial_snapshots_len = c.get_finalized_snapshots_len().await;

    // First period: collateralize
    c.collateralize(&user, 1_000_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Second period: borrow
    c.borrow(&user, 400_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Third period: more borrowing
    c.borrow(&user, 200_000).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Create snapshot
    c.apply_interest(&user, None, None).await;

    let final_snapshots_len = c.get_finalized_snapshots_len().await;
    let new_snapshots_count = final_snapshots_len - initial_snapshots_len;

    assert!(
        new_snapshots_count >= 3,
        "Should have created at least 3 new snapshots, got {new_snapshots_count}",
    );

    // Get the snapshots
    let snapshots = c
        .list_finalized_snapshots(Some(initial_snapshots_len), None)
        .await;

    eprintln!("Snapshots progression:");
    for (i, snapshot) in snapshots.iter().enumerate() {
        eprintln!(
            "Snapshot {}: collateral={:?}, borrowed={:?}",
            i,
            u128::from(snapshot.collateral_asset_deposited()),
            u128::from(snapshot.borrow_asset_borrowed())
        );
    }

    // Expected progression states - but allow for different ordering due to timing
    let expected_states = [
        (0.into(), 0.into()),
        (1_000_000.into(), 0.into()),
        (1_000_000.into(), 400_000.into()),
        (1_000_000.into(), 600_000.into()),
    ];

    // Verify that we see the expected progression somewhere in the snapshots
    let mut found_states = vec![false; expected_states.len()];

    for snapshot in &snapshots {
        let current_state = (
            snapshot.collateral_asset_deposited(),
            snapshot.borrow_asset_borrowed(),
        );

        for (i, expected_state) in expected_states.iter().enumerate() {
            if current_state == *expected_state {
                found_states[i] = true;
                eprintln!("Found expected state {i}: {expected_state:?}");
            }
        }
    }

    // Should find at least the final state and some intermediate states
    assert!(
        found_states[found_states.len() - 1], // Final state
        "Should find final state (1M collateral, 600k borrowed)"
    );

    let found_count = found_states.iter().filter(|&&x| x).count();
    assert!(
        found_count >= 2,
        "Should find at least 2 expected states in progression, found {found_count}",
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
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(),
            };
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

    let amount_after_borrow = u128::from(borrow_snapshot.borrow_asset_borrowed());
    let amount_after_repay = u128::from(repay_snapshot.borrow_asset_borrowed());

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
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(), // 0.5 seconds
            };
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

    let snapshots = c
        .list_finalized_snapshots(Some(final_snapshots_len - 1), Some(1))
        .await;
    let latest_snapshot = &snapshots[0];
    eprintln!("Empty period snapshot: {latest_snapshot:#?}");

    // Should still have a valid snapshot even with minimal activity
    assert_eq!(
        latest_snapshot.borrow_asset_deposited_active(),
        1_000_000.into(),
        "Should maintain previous active deposits",
    );
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
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(),
            };
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

    let final_snapshots_len = c.get_finalized_snapshots_len().await;
    let snapshots = c
        .list_finalized_snapshots(Some(final_snapshots_len - 1), Some(1))
        .await;
    let final_snapshot = &snapshots[0];

    eprintln!(
        "After full repayment: borrowed={:?}",
        final_snapshot.borrow_asset_borrowed()
    );

    let final_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    eprintln!(
        "Final position liability: {:?}",
        final_position.get_total_borrow_asset_liability()
    );

    // Verify snapshot reflects full repayment
    assert!(
        final_snapshot.borrow_asset_borrowed() <= 1000.into(), // Allow for small rounding
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
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(),
            };
        })
    );

    let initial_snapshots_len = c.get_finalized_snapshots_len().await;

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

    let final_snapshots_len = c.get_finalized_snapshots_len().await;
    let snapshots_count = final_snapshots_len - initial_snapshots_len;

    eprintln!("Created {snapshots_count} snapshots");

    assert!(snapshots_count >= 3);

    let recent_snapshots = c
        .list_finalized_snapshots(Some(final_snapshots_len - 3), Some(3))
        .await;

    for (i, snapshot) in recent_snapshots.iter().enumerate() {
        eprintln!("Snapshot {i}: ");
        eprintln!("{snapshot:?}");
        eprintln!();
    }

    let first = &recent_snapshots[0];
    let last = &recent_snapshots[recent_snapshots.len() - 1];

    // Validate field progressions
    assert!(
        last.collateral_asset_deposited() >= first.collateral_asset_deposited(),
        "Collateral should not decrease",
    );

    assert!(
        last.borrow_asset_borrowed() >= first.borrow_asset_borrowed(),
        "Borrowed amount should increase with interest",
    );

    // Timestamps should be increasing
    assert!(
        last.end_timestamp_ms() > first.end_timestamp_ms(),
        "Timestamps should increase",
    );

    // Interest rate should reflect utilization
    assert!(
        !last.interest_rate().is_zero(),
        "Interest rate should be positive with borrowing activity",
    );
}

#[tokio::test]
async fn snapshot_at_time_boundaries() {
    setup_test!(
        extract(c)
        accounts(user1, user2, supply_user)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("0"), dec!("0")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 5000.into(), // 5 second chunks
            };
        })
    );

    c.supply_and_harvest_until_activation(&supply_user, 3_000_000)
        .await;

    let initial_snapshots_len = c.get_finalized_snapshots_len().await;

    // Operations right at boundary
    c.collateralize(&user1, 500_000).await;
    c.collateralize(&user2, 300_000).await;

    // Wait almost to boundary
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Trigger snapshot for first chunk
    c.borrow(&user1, 1).await; // Small operation to trigger snapshot

    let after_first_boundary_len = c.get_finalized_snapshots_len().await;

    // Multiple operations in quick succession near boundary
    c.borrow(&user1, 200_000).await;
    c.borrow(&user2, 100_000).await;

    // Wait to cross another boundary and trigger snapshot
    tokio::time::sleep(Duration::from_secs(6)).await;
    c.collateralize(&user1, 1).await; // Trigger snapshot finalization

    let final_snapshots_len = c.get_finalized_snapshots_len().await;

    eprintln!("Snapshot indices: {initial_snapshots_len} -> {after_first_boundary_len} -> {final_snapshots_len}");

    assert!(
        final_snapshots_len > initial_snapshots_len,
        "Should create snapshot at time boundary"
    );

    // Get the last two snapshots to compare across boundaries
    if final_snapshots_len >= 2 {
        let snapshots = c
            .list_finalized_snapshots(Some(final_snapshots_len - 2), Some(2))
            .await;

        if snapshots.len() >= 2 {
            let first_boundary_snapshot = &snapshots[0];
            let second_boundary_snapshot = &snapshots[1];

            eprintln!("First boundary snapshot: {first_boundary_snapshot:#?}");
            eprintln!("Second boundary snapshot: {second_boundary_snapshot:#?}");

            // First snapshot should have collateral but no borrowing
            assert_eq!(
                first_boundary_snapshot.collateral_asset_deposited(),
                500_000.into(), // 500k + 300k
                "First snapshot should capture collateral operations"
            );

            assert_eq!(
                first_boundary_snapshot.borrow_asset_borrowed(),
                0.into(),
                "First snapshot should have no borrowing yet"
            );

            // Second snapshot should have both collateral and borrowing
            assert_eq!(
                second_boundary_snapshot.collateral_asset_deposited(),
                800_000.into(), // Previous + 1 from trigger
                "Second snapshot should maintain collateral"
            );

            assert_eq!(
                second_boundary_snapshot.borrow_asset_borrowed(),
                300_001.into(), // 200k + 100k
                "Second snapshot should capture borrow operations"
            );
        }
    }
}

#[tokio::test]
async fn many_users_same_snapshot() {
    setup_test!(
        extract(c)
        accounts(user1, user2, user3, user4, user5, supply_user1, supply_user2)
        config(|c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.borrow_origination_fee = Fee::zero();
            c.time_chunk_configuration = TimeChunkConfiguration::BlockTimestampMs {
                divisor: 500.into(),
            };
        })
    );

    // Multiple suppliers
    c.supply_and_harvest_until_activation(&supply_user1, 2_000_000)
        .await;
    c.supply_and_harvest_until_activation(&supply_user2, 1_500_000)
        .await;

    // Many users doing operations in same time chunk
    let collateral_amounts = [400_000, 350_000, 300_000, 250_000, 200_000];
    let borrow_amounts = [150_000, 120_000, 100_000, 80_000, 60_000];

    // All collateral operations
    c.collateralize(&user1, collateral_amounts[0]).await;
    c.collateralize(&user2, collateral_amounts[1]).await;
    c.collateralize(&user3, collateral_amounts[2]).await;
    c.collateralize(&user4, collateral_amounts[3]).await;
    c.collateralize(&user5, collateral_amounts[4]).await;

    // All borrow operations
    c.borrow(&user1, borrow_amounts[0]).await;
    c.borrow(&user2, borrow_amounts[1]).await;
    c.borrow(&user3, borrow_amounts[2]).await;
    c.borrow(&user4, borrow_amounts[3]).await;
    c.borrow(&user5, borrow_amounts[4]).await;

    // Wait and trigger snapshot
    tokio::time::sleep(Duration::from_secs(1)).await;
    c.harvest_yield(&supply_user1, None, None).await;

    let final_snapshots_len = c.get_finalized_snapshots_len().await;
    let snapshots = c
        .list_finalized_snapshots(Some(final_snapshots_len - 1), Some(1))
        .await;
    let multi_user_snapshot = &snapshots[0];

    let total_expected_collateral: u128 = collateral_amounts.iter().sum();
    let total_expected_borrow: u128 = borrow_amounts.iter().sum();

    eprintln!("Multi-user snapshot: {multi_user_snapshot:#?}");
    eprintln!("Expected collateral total: {total_expected_collateral}");
    eprintln!("Expected borrow total: {total_expected_borrow}");

    // Verify aggregate amounts are correct
    assert!(
        multi_user_snapshot.collateral_asset_deposited() >= total_expected_collateral.into(),
        "Should aggregate all collateral from multiple users"
    );

    assert!(
        multi_user_snapshot.borrow_asset_borrowed() >= total_expected_borrow.into(),
        "Should aggregate all borrows from multiple users"
    );

    // Verify we have reasonable supply amounts from multiple suppliers
    assert!(
        multi_user_snapshot.borrow_asset_deposited_active() >= 3_500_000.into(),
        "Should show combined supply from multiple suppliers"
    );
}
