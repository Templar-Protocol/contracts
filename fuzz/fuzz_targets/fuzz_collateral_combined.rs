#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

fuzz_target!(|data: (u32, u128, u128)| {
    let (snapshot_index, collateral_amount, lock_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);

    // Verify initial state
    assert_eq!(
        position.get_total_collateral_amount(),
        CollateralAssetAmount::zero()
    );

    // Set both collateral deposit and liquidation lock
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.liquidation_lock = CollateralAssetAmount::new(lock_amount);

    // Test that total collateral is the sum of both
    let total = position.get_total_collateral_amount();
    let expected_u128 = collateral_amount.saturating_add(lock_amount);

    assert_eq!(u128::from(total), expected_u128);

    // Test individual components are preserved
    assert_eq!(
        u128::from(position.collateral_asset_deposit),
        collateral_amount
    );
    assert_eq!(u128::from(position.liquidation_lock), lock_amount);

    // Test existence logic with both amounts
    if collateral_amount > 0 || lock_amount > 0 {
        assert!(
            position.exists(),
            "Position should exist with any non-zero collateral or lock"
        );
    } else {
        assert!(
            !position.exists(),
            "Position should not exist when both amounts are zero"
        );
    }

    // Test that borrow amounts remain unaffected
    assert_eq!(
        position.get_borrow_asset_principal(),
        BorrowAssetAmount::zero()
    );
    assert_eq!(
        position.get_total_borrow_asset_liability(),
        BorrowAssetAmount::zero()
    );

    // Test math properties: total >= individual components
    assert!(u128::from(total) >= collateral_amount);
    assert!(u128::from(total) >= lock_amount);

    // Test updating one component at a time
    let new_collateral = collateral_amount.saturating_add(1000);
    position.collateral_asset_deposit = CollateralAssetAmount::new(new_collateral);

    let updated_total = position.get_total_collateral_amount();
    let expected_updated = new_collateral.saturating_add(lock_amount);
    assert_eq!(u128::from(updated_total), expected_updated);

    // Test updating the other component
    let new_lock = lock_amount.saturating_add(2000);
    position.liquidation_lock = CollateralAssetAmount::new(new_lock);

    let final_total = position.get_total_collateral_amount();
    let expected_final = new_collateral.saturating_add(new_lock);
    assert_eq!(u128::from(final_total), expected_final);

    // Test clearing both components
    position.collateral_asset_deposit = CollateralAssetAmount::zero();
    position.liquidation_lock = CollateralAssetAmount::zero();

    let cleared_total = position.get_total_collateral_amount();
    assert_eq!(cleared_total, CollateralAssetAmount::zero());
    assert!(
        !position.exists(),
        "Position should not exist after clearing both amounts"
    );
});

