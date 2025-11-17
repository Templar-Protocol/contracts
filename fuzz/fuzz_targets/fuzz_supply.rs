#![no_main]
#[cfg(not(target_arch = "wasm32"))]
use libfuzzer_sys::fuzz_target;
use templar_common::{
    accumulator::Accumulator,
    asset::BorrowAssetAmount,
    incoming_deposit::IncomingDeposit,
    supply::{Deposit, SupplyPosition},
};

fuzz_target!(|data: (u32, u128, u128, u128, u32, u32, u128)| {
    let (
        snapshot_index,
        active_amount,
        incoming_amount_1,
        incoming_amount_2,
        activate_at_1,
        activate_at_2,
        yield_amount,
    ) = data;

    // Fuzz SupplyPosition creation and basic operations
    let mut position = SupplyPosition::new(snapshot_index);

    // Test exists() on new position
    let _ = position.exists();
    let _ = position.can_be_removed();
    // Fuzz deposit structure
    let mut deposit = Deposit {
        active: BorrowAssetAmount::new(active_amount),
        incoming: vec![],
        outgoing: BorrowAssetAmount::zero(),
    };

    // Add incoming deposits
    if incoming_amount_1 > 0 && activate_at_1 > snapshot_index {
        deposit.incoming.push(IncomingDeposit {
            activate_at_snapshot_index: activate_at_1,
            amount: BorrowAssetAmount::new(incoming_amount_1),
        });
    }

    if incoming_amount_2 > 0 && activate_at_2 > snapshot_index && activate_at_2 != activate_at_1 {
        deposit.incoming.push(IncomingDeposit {
            activate_at_snapshot_index: activate_at_2,
            amount: BorrowAssetAmount::new(incoming_amount_2),
        });
    }

    // Test total calculation (should not panic on overflow)
    let _ = deposit.total();

    // Update position with deposit

    // Fuzz yield accumulator
    let yield_acc = Accumulator::new(snapshot_index);
    if yield_amount > 0 {
        // Try to add yield
        let _ = yield_acc.get_total();
    }
    position.borrow_asset_yield = yield_acc;

    // Test total_incoming
    let _ = position.total_incoming();

    // Test exists and can_be_removed after modifications
    let _ = position.exists();
    let _ = position.can_be_removed();

    // Test get methods
    let _ = position.get_deposit();
    let _ = position.get_started_at_block_timestamp_ms();

    // Fuzz edge cases
    let zero_position = SupplyPosition::new(0);
    let _ = zero_position.exists();
    let _ = zero_position.can_be_removed();

    let max_position = SupplyPosition::new(u32::MAX);
    let _ = max_position.exists();

    // Test deposit with max values
    let max_deposit = Deposit {
        active: BorrowAssetAmount::new(u128::MAX / 3),
        incoming: vec![IncomingDeposit {
            activate_at_snapshot_index: u32::MAX - 1,
            amount: BorrowAssetAmount::new(u128::MAX / 3),
        }],
        outgoing: BorrowAssetAmount::new(u128::MAX / 3),
    };
    // This might overflow, which is expected behavior
    let _ = max_deposit.total();
});
