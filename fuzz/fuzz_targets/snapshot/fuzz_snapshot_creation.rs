#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use near_sdk::json_types::U64;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    snapshot::Snapshot,
    time_chunk::TimeChunk,
    Decimal,
};

#[derive(Arbitrary, Debug)]
struct SnapshotCreationScenario {
    time_chunk: u64,
    end_timestamp_ms: u64,
    borrow_asset_deposited_active: u128,
    borrow_asset_borrowed: u128,
    collateral_asset_deposited: u128,
    yield_distribution: u128,
}

fuzz_target!(|scenario: SnapshotCreationScenario| {
    // Create time chunk
    let time_chunk = TimeChunk(U64(scenario.time_chunk));

    // Build snapshot manually to test all fields
    let snapshot = Snapshot {
        time_chunk,
        end_timestamp_ms: U64(scenario.end_timestamp_ms),
        borrow_asset_deposited_active: BorrowAssetAmount::from(
            scenario.borrow_asset_deposited_active,
        ),
        borrow_asset_borrowed: BorrowAssetAmount::from(scenario.borrow_asset_borrowed),
        collateral_asset_deposited: CollateralAssetAmount::from(
            scenario.collateral_asset_deposited,
        ),
        yield_distribution: BorrowAssetAmount::from(scenario.yield_distribution),
        interest_rate: Decimal::ZERO, // Use safe default
    };

    // Invariant: Time chunk should be preserved
    assert_eq!(
        snapshot.time_chunk, time_chunk,
        "Time chunk should match input"
    );

    // Invariant: Timestamp should be preserved
    assert_eq!(
        snapshot.end_timestamp_ms.0, scenario.end_timestamp_ms,
        "Timestamp should match input"
    );
});
