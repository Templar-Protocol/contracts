#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{asset::BorrowAssetAmount, borrow::BorrowPosition};

// Tests overflow handling with very large amounts
fuzz_target!(|data: (u32, u128, u128)| {
    let (snapshot_index, principal_divisor, inflight_divisor) = data;

    let principal_div = match principal_divisor % 2 {
        0 => 1,
        _ => principal_divisor,
    };

    let inflight_div = match inflight_divisor % 2 {
        0 => 1,
        _ => inflight_divisor,
    };

    let borrow_asset_principal = u128::MAX / principal_div;
    let borrow_asset_inflight = u128::MAX / inflight_div;

    let mut position = BorrowPosition::new(snapshot_index);

    position.borrow_asset_principal = BorrowAssetAmount::new(borrow_asset_principal);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(borrow_asset_inflight);

    // Should not panic - test saturating arithmetic
    let liability = position.get_total_borrow_asset_liability();
    let position_principal = position.get_borrow_asset_principal();

    // Invariant should still hold even with large values
    assert!(u128::from(liability) >= u128::from(position_principal));
});
