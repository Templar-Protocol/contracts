#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

fuzz_target!(|data: (u32, u128)| {
    let (snapshot_index, collateral_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);

    // Verify initial state - zero collateral
    assert_eq!(
        position.get_total_collateral_amount(),
        CollateralAssetAmount::zero()
    );
    assert_eq!(
        position.collateral_asset_deposit,
        CollateralAssetAmount::zero()
    );

    // Set collateral deposit
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);

    // Test that total collateral equals deposit (no liquidation lock)
    let total = position.get_total_collateral_amount();
    assert_eq!(total, position.collateral_asset_deposit);
    assert_eq!(u128::from(total), collateral_amount);

    // Test existence logic
    if collateral_amount > 0 {
        assert!(
            position.exists(),
            "Position should exist with non-zero collateral"
        );
    } else {
        assert!(
            !position.exists(),
            "Position should not exist with zero collateral"
        );
    }

    // Verify liquidation lock remains zero
    assert_eq!(position.liquidation_lock, CollateralAssetAmount::zero());

    // Verify borrow amounts remain zero (collateral operations don't affect them)
    assert_eq!(
        position.get_borrow_asset_principal(),
        BorrowAssetAmount::zero()
    );
    assert_eq!(
        position.get_total_borrow_asset_liability(),
        BorrowAssetAmount::zero()
    );
    assert_eq!(position.borrow_asset_principal, BorrowAssetAmount::zero());
    assert_eq!(position.borrow_asset_in_flight, BorrowAssetAmount::zero());

    // Test that we can update the deposit
    let new_amount = collateral_amount.saturating_add(1000);
    position.collateral_asset_deposit = CollateralAssetAmount::new(new_amount);

    let updated_total = position.get_total_collateral_amount();
    assert_eq!(u128::from(updated_total), new_amount);

    if new_amount > 0 {
        assert!(
            position.exists(),
            "Position should exist after deposit update"
        );
    }
});

