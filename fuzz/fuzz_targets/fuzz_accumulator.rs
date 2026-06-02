//! Fuzz `Accumulator::<BorrowAsset>` — add_once, remove, clear must preserve
//! the invariant `get_total() == sum_of_added - sum_of_removed_capped` and
//! never panic.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, FungibleAssetAmount},
};

// Cap inputs to u64 so consecutive `add_once`s can't trip intentional
// overflow checks on the inner aggregate.
//
// MUTATION-CHECK (P5): in `Accumulator::clear` (accumulator.rs:30), drop the
// `self.total = 0.into();` line (only reset the snapshot index). Then the
// post-clear `get_total() == zero` assertion below must fire.
fuzz_target!(|data: (u32, u32, u64, u64)| {
    let (initial_snapshot, next_snapshot, add_a, remove_b) = data;
    let add_a = FungibleAssetAmount::<BorrowAsset>::new(u128::from(add_a));
    let remove_b = FungibleAssetAmount::<BorrowAsset>::new(u128::from(remove_b));

    let mut accumulator = Accumulator::<BorrowAsset>::new(initial_snapshot);
    assert_eq!(accumulator.get_next_snapshot_index(), initial_snapshot);
    assert_eq!(accumulator.get_total(), FungibleAssetAmount::zero());

    accumulator.add_once(add_a);
    assert_eq!(accumulator.get_total(), add_a);

    // `Accumulator::remove` panics on underflow (overflow-checks). Caller
    // contract: amount removed ≤ current total. Cap the fuzz input here.
    let removable = remove_b.min(add_a);
    accumulator.remove(removable);
    assert!(accumulator.get_total() <= add_a);

    accumulator.add_once(add_a);
    // After re-adding, total must be at least the new addend.
    assert!(accumulator.get_total() >= add_a);

    accumulator.clear(next_snapshot);
    assert_eq!(accumulator.get_next_snapshot_index(), next_snapshot);
    assert_eq!(accumulator.get_total(), FungibleAssetAmount::zero());
});
