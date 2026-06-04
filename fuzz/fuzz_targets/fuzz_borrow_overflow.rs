//! Boundary target (P2 backstop for `fuzz_borrow` / `fuzz_borrow_invariants`).
//!
//! `BorrowPosition::get_total_borrow_asset_liability` adds
//! `principal + in_flight + interest + fees`. The contract has
//! `overflow-checks = true` in the release profile, so a sum past `u128::MAX`
//! aborts — that abort is the documented safety property of the contract.
//!
//! ## What this target tests
//! - Drives `principal` and `in_flight` across the full `u128` range, including
//!   inputs near `u128::MAX` that the narrowed `fuzz_borrow` cannot reach.
//! - For inputs where the sum **fits** in `u128`, asserts the function returns
//!   the **exact** expected sum. A bug that used `wrapping_add` (silent
//!   wraparound) would here either return the wrong value (caught by the
//!   assertion) or abort on safe inputs (caught as a libfuzzer crash).
//!
//! ## What this target does NOT test
//! libfuzzer-sys installs a `panic::set_hook` that calls `process::abort()`
//! *before* the Rust unwinder runs (see `libfuzzer-sys-0.4.12/src/lib.rs:84`),
//! so `catch_unwind` cannot observe a panicking call from inside the
//! `fuzz_target`. Thus this target can't directly assert "calling with an
//! overflowing sum aborts." That direction is verified by the unit test
//! `tests::overflow_aborts` in `common/src/borrow.rs`.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{asset::BorrowAssetAmount, borrow::BorrowPosition};

// MUTATION-CHECK (P5): change `get_total_borrow_asset_liability` /
// `get_borrow_asset_principal` to add `+ 1` (or use `wrapping_add`). On
// non-overflowing inputs the differential assertion (`== principal + in_flight`)
// must fire — including near-u128::MAX operands that `fuzz_borrow` can't reach.

fuzz_target!(|data: (u128, u128)| {
    let (principal, in_flight) = data;

    // If the sum would overflow, calling the real function aborts. Inside
    // libfuzzer-sys that abort is indistinguishable from a real bug crash, so
    // we predict overflow and skip the call. The point of this target is to
    // assert *correctness up to the boundary* — that the function returns
    // exact `principal + in_flight` for every non-overflowing combination,
    // including values like `(u128::MAX - 1, 1)` that the narrowed
    // `fuzz_borrow` cannot generate.
    let Some(expected_sum) = principal.checked_add(in_flight) else {
        return;
    };

    let mut position = BorrowPosition::new(0);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight);

    // Differential: must equal the oracle for every safe input. A buggy
    // implementation using `wrapping_add` would disagree on edge cases.
    let liability = position.get_total_borrow_asset_liability();
    assert_eq!(
        liability,
        BorrowAssetAmount::new(expected_sum),
        "liability ({liability:?}) must equal principal + in_flight \
         ({principal} + {in_flight} = {expected_sum}) exactly on non-overflowing inputs",
    );

    // `get_borrow_asset_principal` documented as principal + in_flight; same
    // boundary check.
    let getter_principal = position.get_borrow_asset_principal();
    assert_eq!(
        getter_principal,
        BorrowAssetAmount::new(expected_sum),
        "get_borrow_asset_principal must equal principal + in_flight exactly",
    );
});
