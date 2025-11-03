#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::BorrowAssetAmount,
    borrow::BorrowPosition,
};

// Tests only in_flight amount operations
fuzz_target!(|data: (u32, u128)| {
    let (snapshot_index, in_flight_amount) = data;

    let mut position = BorrowPosition::new(snapshot_index);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);

    let liability = position.get_total_borrow_asset_liability();

    // Test existence with in_flight only
    if in_flight_amount > 0 {
        assert!(position.exists());
        assert!(!liability.is_zero());
    } else {
        assert!(!position.exists());
        assert_eq!(liability, BorrowAssetAmount::zero());
    }

    // Principal should remain zero
    assert_eq!(position.get_borrow_asset_principal(), BorrowAssetAmount::zero());
});
