#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests position with all amounts set
fuzz_target!(|data: (u32, u128, u128, u128, u128)| {
    let (snapshot_index, collateral_amount, principal_amount, in_flight_amount, lock_divisor) =
        data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);

    let lock_amount = if lock_divisor > 0 {
        collateral_amount / lock_divisor.max(10)
    } else {
        0
    };
    position.liquidation_lock = CollateralAssetAmount::new(lock_amount);

    // All getters should work
    let liability = position.get_total_borrow_asset_liability();
    let collateral = position.get_total_collateral_amount();
    let principal = position.get_borrow_asset_principal();

    // Test core invariants with all amounts set
    assert!(u128::from(liability) >= u128::from(principal));
    assert!(u128::from(collateral) >= collateral_amount);
    assert_eq!(principal, position.borrow_asset_principal);
});
