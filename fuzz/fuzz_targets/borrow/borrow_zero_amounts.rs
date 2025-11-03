#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests BorrowPosition with all zero amounts
fuzz_target!(|snapshot_index: u32| {
    let position = BorrowPosition::new(snapshot_index);

    // All amounts should be zero
    assert_eq!(position.get_borrow_asset_principal(), BorrowAssetAmount::zero());
    assert_eq!(position.get_total_borrow_asset_liability(), BorrowAssetAmount::zero());
    assert_eq!(position.get_total_collateral_amount(), CollateralAssetAmount::zero());

    // Position should not exist
    assert!(!position.exists());

    // Test that setting zero amounts explicitly doesn't change behavior
    let mut mut_position = BorrowPosition::new(snapshot_index);
    mut_position.collateral_asset_deposit = CollateralAssetAmount::zero();
    mut_position.borrow_asset_principal = BorrowAssetAmount::zero();
    mut_position.borrow_asset_in_flight = BorrowAssetAmount::zero();
    mut_position.liquidation_lock = CollateralAssetAmount::zero();

    assert!(!mut_position.exists());
    assert_eq!(mut_position, position);
});
