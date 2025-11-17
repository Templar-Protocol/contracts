#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus, LiquidationReason},
};

// Tests BorrowStatus enum and timestamp operations
fuzz_target!(|data: (u32, u64, u8)| {
    let (snapshot_index, timestamp_ms, status_selector) = data;

    let mut position = BorrowPosition::new(snapshot_index);

    // Test timestamp operations
    if timestamp_ms > 0 {
        position.started_at_block_timestamp_ms = Some(near_sdk::json_types::U64(timestamp_ms));
    }

    // Test BorrowStatus enum variants
    let status = match status_selector % 4 {
        0 => BorrowStatus::Healthy,
        1 => BorrowStatus::MaintenanceRequired,
        2 => BorrowStatus::Liquidation(LiquidationReason::Undercollateralization),
        _ => BorrowStatus::Liquidation(LiquidationReason::Expiration),
    };

    // Test enum operations
    let cloned_status = status;
    assert_eq!(status, cloned_status);

    // Test enum comparisons
    let healthy = BorrowStatus::Healthy;
    let maintenance = BorrowStatus::MaintenanceRequired;
    assert!(healthy < maintenance);

    // Test liquidation reasons
    let under_reason = LiquidationReason::Undercollateralization;
    let exp_reason = LiquidationReason::Expiration;
    assert_ne!(under_reason, exp_reason);

    // Position should remain empty throughout enum testing
    assert!(!position.exists());
    assert_eq!(
        position.get_borrow_asset_principal(),
        BorrowAssetAmount::zero()
    );
    assert_eq!(
        position.get_total_collateral_amount(),
        CollateralAssetAmount::zero()
    );
});

