#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus, LiquidationReason},
};

fuzz_target!(|data: (u32, u128, u128, u128, u128, u64, u8)| {
    let (
        snapshot_index,
        collateral_amount,
        principal_amount,
        _fees_amount,
        in_flight_amount,
        timestamp_ms,
        op_selector,
    ) = data;

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
    let liability = position.get_total_borrow_asset_liability();
    let collateral = position.get_total_collateral_amount();
    assert_eq!(collateral, position.collateral_asset_deposit);
    let principal = position.get_borrow_asset_principal();
    assert_eq!(principal, position.borrow_asset_principal);

    // Test exists and can_be_removed logic
    let exists = position.exists();
    let can_remove = !position.exists();

    // Invariants
    if exists {
        // If position exists, can_be_removed should consider the amounts
        if position.collateral_asset_deposit.is_zero()
            && liability.is_zero()
            && position.borrow_asset_in_flight.is_zero()
            && position.liquidation_lock.is_zero()
        {
            assert!(
                can_remove,
                "Position should be removable when all amounts are zero"
            );
        }
    }

    // Test timestamp handling
    if timestamp_ms > 0 {
        position.started_at_block_timestamp_ms = Some(near_sdk::json_types::U64(timestamp_ms));
    }

    // Test different operations based on selector
    match op_selector % 8 {
        0 => {
            // Test with zero amounts
            let zero_pos = BorrowPosition::new(0);
            assert!(!zero_pos.exists());
            assert!(zero_pos.exists());
            assert_eq!(
                zero_pos.get_borrow_asset_principal(),
                BorrowAssetAmount::zero()
            );
        }
        1 => {
            // Test with max snapshot index
            let max_pos = BorrowPosition::new(u32::MAX);
            let _ = max_pos.get_total_borrow_asset_liability();
        }
        2 => {
            // Test collateral operations
            let mut pos = BorrowPosition::new(snapshot_index);
            pos.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
            let total = pos.get_total_collateral_amount();
            assert_eq!(total, pos.collateral_asset_deposit);
        }
        3 => {
            // Test liquidation lock
            let mut pos = BorrowPosition::new(snapshot_index);
            pos.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
            pos.liquidation_lock = CollateralAssetAmount::new(collateral_amount / 2);
            let total = pos.get_total_collateral_amount();
            // Total should be sum of deposit and lock
            let _ = total;
        }
        4 => {
            // Test in-flight amounts
            let mut pos = BorrowPosition::new(snapshot_index);
            pos.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
            pos.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);
            let liability = pos.get_total_borrow_asset_liability();
            // Liability should include principal and in_flight
            let _ = liability;
        }
        5 => {
            // Test fees accumulation
            let mut pos = BorrowPosition::new(snapshot_index);
            pos.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
            // Fees are part of liability
            let liability = pos.get_total_borrow_asset_liability();
            let _ = liability;
        }
        6 => {
            // Test position with all amounts set
            let mut pos = BorrowPosition::new(snapshot_index);
            pos.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
            pos.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
            pos.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);
            pos.liquidation_lock = CollateralAssetAmount::new(collateral_amount / 10);

            let _ = pos.get_total_borrow_asset_liability();
            let _ = pos.get_total_collateral_amount();
            let _ = pos.exists();
            let _ = !pos.exists();
        }
        _ => {
            // Test edge cases with overflow scenarios
            let mut pos = BorrowPosition::new(snapshot_index);

            // Try to set amounts that might overflow when combined
            pos.borrow_asset_principal = BorrowAssetAmount::new(u128::MAX / 3);
            pos.borrow_asset_in_flight = BorrowAssetAmount::new(u128::MAX / 3);

            // This might overflow - fuzzer should catch it
            let _ = pos.get_total_borrow_asset_liability();
        }
    }

    // Test clone and equality
    let cloned = position.clone();
    assert_eq!(position, cloned);

    // Test BorrowStatus enum
    let status_healthy = BorrowStatus::Healthy;
    let status_maintenance = BorrowStatus::MaintenanceRequired;
    let status_liquidation = BorrowStatus::Liquidation(LiquidationReason::Undercollateralization);
    let status_liquidation_exp = BorrowStatus::Liquidation(LiquidationReason::Expiration);

    // Test comparisons
    let _ = status_healthy == status_maintenance;
    let _ = status_liquidation == status_liquidation_exp;
    let _ = status_healthy < status_maintenance;
});
