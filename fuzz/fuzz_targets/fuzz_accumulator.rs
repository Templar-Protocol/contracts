#![no_main]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, FungibleAssetAmount},
};

fuzz_target!(|data: (u32, u128)| {
    let (acc, amount) = data;
    let amount_fungible = FungibleAssetAmount::new(amount);
    let mut accumulator = Accumulator::<BorrowAsset>::new(acc);
    // Initial state assertions
    assert_eq!(accumulator.get_next_snapshot_index(), acc);
    assert_eq!(accumulator.get_total(), 0.into());

    // Add once
    let _ = accumulator.add_once(amount_fungible);
    assert!(accumulator.get_total() >= 0.into());

    // Remove
    let _ = accumulator.remove(amount_fungible);
    assert!(accumulator.get_total() <= amount_fungible);

    // Add again
    let _ = accumulator.add_once(amount_fungible);

    () = accumulator.clear(acc);
});
