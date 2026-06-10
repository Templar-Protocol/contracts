#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

fuzz_target!(|data: (u32, bool)| {
    let (snapshot_index, use_exact_max) = data;

    let mut position = BorrowPosition::new(snapshot_index);

    // Test with large values that could cause overflow
    let large_value = if use_exact_max {
        u128::MAX / 2 // Exactly half of max
    } else {
        u128::MAX / 3 // One third of max
    };

    // Set both collateral deposit and liquidation lock to large values
    position.collateral_asset_deposit = CollateralAssetAmount::new(large_value);
    position.liquidation_lock = CollateralAssetAmount::new(large_value);

    // This should not panic due to saturating arithmetic
    let total = position.get_total_collateral_amount();

    // Total should be at least as large as each component
    assert!(u128::from(total) >= large_value);

    // Position should exist with these large values
    assert!(
        position.exists(),
        "Position should exist with large collateral values"
    );

    // Test individual components are preserved
    assert_eq!(u128::from(position.collateral_asset_deposit), large_value);
    assert_eq!(u128::from(position.liquidation_lock), large_value);

    // Test that the total is the saturated sum
    let expected_total = large_value.saturating_add(large_value);
    assert_eq!(u128::from(total), expected_total);

    // Test edge case: setting one component to u128::MAX
    position.collateral_asset_deposit = CollateralAssetAmount::new(u128::MAX);
    position.liquidation_lock = CollateralAssetAmount::new(1);

    let max_total = position.get_total_collateral_amount();
    // Should saturate at u128::MAX, not overflow
    assert_eq!(u128::from(max_total), u128::MAX);
    assert!(position.exists());

    // Test other extreme: both at u128::MAX
    position.liquidation_lock = CollateralAssetAmount::new(u128::MAX);

    let double_max_total = position.get_total_collateral_amount();
    // Should still saturate at u128::MAX
    assert_eq!(u128::from(double_max_total), u128::MAX);
    assert!(position.exists());

    // Verify borrow amounts remain zero even with extreme collateral values
    assert_eq!(
        position.get_borrow_asset_principal(),
        BorrowAssetAmount::zero()
    );
    assert_eq!(
        position.get_total_borrow_asset_liability(),
        BorrowAssetAmount::zero()
    );

    // Test that we can still clear the amounts after overflow scenarios
    position.collateral_asset_deposit = CollateralAssetAmount::zero();
    position.liquidation_lock = CollateralAssetAmount::zero();

    let cleared_total = position.get_total_collateral_amount();
    assert_eq!(cleared_total, CollateralAssetAmount::zero());
    assert!(
        !position.exists(),
        "Position should not exist after clearing extreme amounts"
    );
});

