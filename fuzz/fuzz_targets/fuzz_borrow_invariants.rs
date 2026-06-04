//! Fuzz `BorrowPosition` invariants. Inputs are `u64` so the liability sum
//! `principal + in_flight + interest + fees` cannot trip `u128`'s intentional
//! overflow check (P2: targeted bound). The boundary itself is fuzzed in
//! `fuzz_borrow_overflow`, which is the P2 backstop for this narrowing.
//!
//! Assertions are restricted to properties a buggy implementation could
//! actually violate (P2); tautological reads-of-fields-we-just-set are not
//! kept here.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// MUTATION-CHECK (P5): in `BorrowPosition::get_total_borrow_asset_liability`
// (borrow.rs:95), drop the `+ self.borrow_asset_in_flight` term. Then the
// `liability == principal + in_flight` exact-equality assertion must fire.

fuzz_target!(|data: (u32, u64, u64, u64, u64, u64, bool)| {
    let (snapshot_index, c1, c2, b1, _b2, in_flight, has_timestamp) = data;
    let (c1, c2, b1, in_flight) = (
        u128::from(c1),
        u128::from(c2),
        u128::from(b1),
        u128::from(in_flight),
    );

    let mut position = BorrowPosition::new(snapshot_index);
    position.collateral_asset_deposit = CollateralAssetAmount::new(c1);
    position.borrow_asset_principal = BorrowAssetAmount::new(b1);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight);

    // Property: with no accrued interest or fees, total liability equals the
    // sum of principal + in_flight. A bug that double-counted or dropped one
    // would violate this.
    let liability = position.get_total_borrow_asset_liability();
    let expected_liability = position.borrow_asset_principal + position.borrow_asset_in_flight;
    assert_eq!(
        liability, expected_liability,
        "fresh position: liability must equal principal + in_flight exactly",
    );

    // Property: `exists()` is a tight bi-implication on the 3 contributing
    // fields. The fuzzer doesn't set `collateral_asset_in_flight`, so the
    // disjunction here is exact.
    let expected_exists = !position.collateral_asset_deposit.is_zero() || !liability.is_zero();
    assert_eq!(position.exists(), expected_exists);

    if has_timestamp {
        position.started_at_block_timestamp_ms = Some(near_sdk::json_types::U64(1_000_000));
    }

    // Property: monotonicity — adding in_flight to a position never decreases
    // the total liability. (Buggy code could e.g. mis-account in_flight as a
    // credit.)
    let mut seq = BorrowPosition::new(snapshot_index);
    seq.borrow_asset_principal = BorrowAssetAmount::new(b1);
    let l_before = seq.get_total_borrow_asset_liability();
    seq.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight);
    let l_after = seq.get_total_borrow_asset_liability();
    assert!(
        l_after >= l_before,
        "adding in_flight must not decrease total liability ({l_before:?} -> {l_after:?})",
    );

    // Property: a fresh `BorrowPosition::new(idx)` is empty.
    let fresh = BorrowPosition::new(snapshot_index);
    assert!(!fresh.exists(), "fresh position must not exist");
    assert!(fresh.get_total_borrow_asset_liability().is_zero());
    assert!(fresh.get_total_collateral_amount().is_zero());

    // Property: clone equality, and inequality after mutation. A buggy `Eq`
    // could pass clone-equality but miss the mutation (or vice versa).
    let clone = position.clone();
    assert_eq!(position, clone);
    let mut modified = position.clone();
    modified.collateral_asset_deposit = CollateralAssetAmount::new(c2);
    if c1 != c2 {
        assert_ne!(
            position, modified,
            "mutated position must not equal original"
        );
    }
});
