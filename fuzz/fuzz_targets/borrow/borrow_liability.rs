#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests borrow liability calculation (principal + in_flight)
fuzz_target!(|data: (u32, u128, u128)| {
    let (snapshot_index, principal_amount, in_flight_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);

    let liability = position.get_total_borrow_asset_liability();
    let principal = position.get_borrow_asset_principal();

    // Test core invariant: liability >= principal
    assert!(u128::from(liability) >= u128::from(principal));

    // Test existence with borrow amounts
    if principal_amount > 0 || in_flight_amount > 0 {
        assert!(position.exists());
    } else {
        assert!(!position.exists());
    }

    // Verify collateral remains zero
    assert_eq!(position.get_total_collateral_amount(), CollateralAssetAmount::zero());
});

