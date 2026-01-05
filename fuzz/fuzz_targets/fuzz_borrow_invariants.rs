#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

fuzz_target!(|data: (u32, u128, u128, u128, u128, u128, bool)| {
    let (snapshot_index, collateral_1, collateral_2, borrow_1, _borrow_2, in_flight, has_timestamp) =
        data;

    // Test collateral calculations
    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_1);

    let total_collateral = position.get_total_collateral_amount();

    // Invariant: total collateral >= deposit
    assert!(
        total_collateral >= position.collateral_asset_deposit,
        "Total collateral should be >= deposit"
    );

    // Test borrow liability calculations
    position.borrow_asset_principal = BorrowAssetAmount::new(borrow_1);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight);

    let total_liability = position.get_total_borrow_asset_liability();

    // Invariant: total liability >= principal
    assert!(
        total_liability >= position.borrow_asset_principal,
        "Total liability should be >= principal"
    );

    // Invariant: total liability >= in_flight
    assert!(
        total_liability >= position.borrow_asset_in_flight,
        "Total liability should be >= in_flight"
    );

    // Test exists logic
    if !position.collateral_asset_deposit.is_zero() || !total_liability.is_zero() {
        assert!(
            position.exists(),
            "Position should exist with non-zero amounts"
        );
    }

    // Test can_be_removed logic
    if position.collateral_asset_deposit.is_zero()
        && total_liability.is_zero()
        && position.borrow_asset_in_flight.is_zero()
    {
        assert!(
            !position.exists(),
            "Position should be removable when all zero"
        );
    } else {
        assert!(
            position.exists(),
            "Position should not be removable with non-zero amounts"
        );
    }

    // Test timestamp handling
    if has_timestamp {
        position.started_at_block_timestamp_ms = Some(near_sdk::json_types::U64(1_000_000));
        assert!(position.started_at_block_timestamp_ms.is_some());
    }

    // Test multiple operations in sequence
    let mut seq_position = BorrowPosition::new(snapshot_index);

    // Step 1: Add collateral
    seq_position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_1);
    let step1_collateral = seq_position.get_total_collateral_amount();
    assert_eq!(step1_collateral, collateral_1.into());

    // Step 2: Add borrow
    seq_position.borrow_asset_principal = BorrowAssetAmount::new(borrow_1);
    let step2_liability = seq_position.get_total_borrow_asset_liability();
    assert!(step2_liability >= borrow_1.into());

    // Step 3: Add in_flight
    seq_position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight);
    let step3_liability = seq_position.get_total_borrow_asset_liability();
    assert!(step3_liability >= step2_liability);

    // Test edge cases

    // Edge case 1: Maximum values that don't overflow
    let mut max_pos = BorrowPosition::new(snapshot_index);
    max_pos.collateral_asset_deposit = CollateralAssetAmount::new(u128::MAX / 2);
    let _ = max_pos.get_total_collateral_amount(); // Should not panic

    // Edge case 2: Zero position
    let zero_pos = BorrowPosition::new(0);
    assert_eq!(zero_pos.get_total_collateral_amount(), 0.into());
    assert_eq!(zero_pos.get_total_borrow_asset_liability(), 0.into());
    assert!(!zero_pos.exists());
    assert!(zero_pos.exists());

    // Edge case 3: Only fees, no principal
    let mut fee_pos = BorrowPosition::new(snapshot_index);
    fee_pos.borrow_asset_principal = BorrowAssetAmount::zero();
    let fee_liability = fee_pos.get_total_borrow_asset_liability();
    // Liability should still be calculable even with zero principal
    let _ = fee_liability;

    // Edge case 4: Only in_flight, no principal
    let mut flight_pos = BorrowPosition::new(snapshot_index);
    flight_pos.borrow_asset_in_flight = BorrowAssetAmount::new(borrow_1);
    let flight_liability = flight_pos.get_total_borrow_asset_liability();
    assert!(flight_liability >= borrow_1.into());

    // Test equality and cloning
    let original = position.clone();
    assert_eq!(position, original);

    // Modify and test inequality
    let mut modified = position.clone();
    modified.collateral_asset_deposit = CollateralAssetAmount::new(collateral_2);
    if collateral_1 != collateral_2 {
        assert_ne!(position, modified);
    }
});
