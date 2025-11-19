#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests position existence logic with any combination of amounts
fuzz_target!(|data: (u32, u128, u128, u128, u128)| {
    let (snapshot_index, collateral_amount, principal_amount, in_flight_amount, lock_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);
    position.liquidation_lock = CollateralAssetAmount::new(lock_amount);

    let exists = position.exists();
    let has_any_amount =
        collateral_amount > 0 || principal_amount > 0 || in_flight_amount > 0 || lock_amount > 0;

    // Core existence logic
    assert_eq!(exists, has_any_amount);

    // Test state transitions - clear all amounts
    position.collateral_asset_deposit = CollateralAssetAmount::zero();
    position.borrow_asset_principal = BorrowAssetAmount::zero();
    position.borrow_asset_in_flight = BorrowAssetAmount::zero();
    position.liquidation_lock = CollateralAssetAmount::zero();

    assert!(!position.exists());
});
