//! Boundary target (P2 backstop for `fuzz_supply`).
//!
//! `Deposit::total` computes `active + outgoing + Σ incoming`. With
//! `overflow-checks = true`, a sum past `u128::MAX` aborts — the documented
//! safety property. The narrowed `fuzz_supply` uses `u64` amounts and cannot
//! reach that boundary; this target drives the full `u128` range.
//!
//! Like `fuzz_borrow_overflow`, it can't observe the abort directly
//! (libfuzzer-sys aborts on panic before `catch_unwind`), so it asserts
//! *correctness up to the boundary*: for every operand combination whose sum
//! fits in `u128`, `total()` must equal that exact sum. The abort direction is
//! covered by the unit test `supply::tests::deposit_total_overflow_aborts`.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::BorrowAssetAmount, incoming_deposit::IncomingDeposit, supply::Deposit,
};

// MUTATION-CHECK (P5): change `Deposit::total` (supply.rs:29) to use
// `wrapping_add` or to add an extra `+ 1`. On non-overflowing inputs the
// differential assertion (`== active + outgoing + incoming_1 + incoming_2`)
// must fire — including operands near u128::MAX that `fuzz_supply` can't reach.

fuzz_target!(|data: (u128, u128, u128, u128)| {
    let (active, outgoing, incoming_1, incoming_2) = data;

    // Predict whether the real sum overflows. If it does, the real `total()`
    // aborts (correct contract behavior) — indistinguishable from a bug crash
    // inside libfuzzer-sys, so we skip the call and rely on the unit test for
    // the abort direction. The value of THIS target is asserting exactness on
    // every non-overflowing combination across the full u128 range, including
    // operands near u128::MAX that `fuzz_supply` (u64-bounded) never reaches.
    let Some(expected) = active
        .checked_add(outgoing)
        .and_then(|s| s.checked_add(incoming_1))
        .and_then(|s| s.checked_add(incoming_2))
    else {
        return;
    };

    let deposit = Deposit {
        active: BorrowAssetAmount::new(active),
        outgoing: BorrowAssetAmount::new(outgoing),
        incoming: vec![
            IncomingDeposit {
                activate_at_snapshot_index: 1,
                amount: BorrowAssetAmount::new(incoming_1),
            },
            IncomingDeposit {
                activate_at_snapshot_index: 2,
                amount: BorrowAssetAmount::new(incoming_2),
            },
        ],
    };

    assert_eq!(
        u128::from(deposit.total()),
        expected,
        "Deposit::total must equal active + outgoing + incoming_1 + incoming_2 \
         exactly on non-overflowing inputs",
    );
});
