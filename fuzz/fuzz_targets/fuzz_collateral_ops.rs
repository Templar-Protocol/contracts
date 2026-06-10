#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

fuzz_target!(|data: (u32, u128, u128, u128, u128,)| {
    let (snapshot_index, collateral_amount, principal_amount, _fees_amount, in_flight_amount) =
        data;

    // Create a new borrow position
    let mut position = BorrowPosition::new(snapshot_index);

    // Test basic getters on empty position
    let _ = position.get_borrow_asset_principal();
    let _ = position.get_total_borrow_asset_liability();
    let _ = position.get_total_collateral_amount();
    let _ = !position.exists();
    let _ = position.exists();

    // Fuzz setting various amounts
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);
    position.liquidation_lock = CollateralAssetAmount::new(0); // Start with no lock

    // Test getters with populated values
    let collateral = position.get_total_collateral_amount();
    assert_eq!(collateral, position.collateral_asset_deposit);
    let principal = position.get_borrow_asset_principal();
    assert_eq!(principal, position.borrow_asset_principal);

    // Test collateral operations
    let mut pos = BorrowPosition::new(snapshot_index);
    pos.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    let total = pos.get_total_collateral_amount();
    assert_eq!(total, pos.collateral_asset_deposit);
});
