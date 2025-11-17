#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests BorrowPosition with maximum snapshot index
fuzz_target!(|data: (u128, u128, u128)| {
    let (collateral_amount, principal_amount, in_flight_amount) = data;

    let mut position = BorrowPosition::new(u32::MAX);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);

    // All operations should work normally with max snapshot
    let liability = position.get_total_borrow_asset_liability();
    let collateral = position.get_total_collateral_amount();
    let principal = position.get_borrow_asset_principal();

    // Same invariants should hold
    assert!(u128::from(liability) >= u128::from(principal));
    assert_eq!(u128::from(collateral), collateral_amount);
});
