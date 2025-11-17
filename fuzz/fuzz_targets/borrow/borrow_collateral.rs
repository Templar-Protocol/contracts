#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// Tests only collateral deposit field operations
fuzz_target!(|data: (u32, u128)| {
    let (snapshot_index, collateral_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    
    let total = position.get_total_collateral_amount();
    assert_eq!(total, position.collateral_asset_deposit);

    // Test existence with collateral
    if collateral_amount > 0 {
        assert!(position.exists());
    } else {
        assert!(!position.exists());
    }

    // Verify other amounts remain zero
    assert_eq!(position.get_borrow_asset_principal(), BorrowAssetAmount::zero());
    assert_eq!(position.get_total_borrow_asset_liability(), BorrowAssetAmount::zero());
    assert_eq!(position.liquidation_lock, CollateralAssetAmount::zero());
});
