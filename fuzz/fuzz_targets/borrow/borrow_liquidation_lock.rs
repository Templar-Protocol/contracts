#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::CollateralAssetAmount,
    borrow::BorrowPosition,
};

// Tests liquidation_lock field operations
fuzz_target!(|data: (u32, u128, u128)| {
    let (snapshot_index, collateral_amount, lock_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.liquidation_lock = CollateralAssetAmount::new(lock_amount);

    let total = position.get_total_collateral_amount();
    let expected_total = collateral_amount.saturating_add(lock_amount);
    
    assert_eq!(u128::from(total), expected_total);

    // Test existence with liquidation lock
    if collateral_amount > 0 || lock_amount > 0 {
        assert!(position.exists());
    } else {
        assert!(!position.exists());
    }
});
