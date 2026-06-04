//! Fuzz `BorrowPosition` getters. Amounts capped at `u64` so the liability sum
//! cannot trip the contract's intentional `u128` overflow check (P2:
//! targeted bound). The boundary is fuzzed in `fuzz_borrow_overflow`, which is
//! the P2 backstop.
//!
//! Assertions are restricted to properties a buggy implementation could
//! actually violate (P2) — see comments per assertion.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
};

// MUTATION-CHECK (P5): in `BorrowPosition::get_borrow_asset_principal`
// (borrow.rs:92), drop the `+ self.borrow_asset_in_flight` term. Then the
// `getter_principal == principal + in_flight` assertion below must fire.

fuzz_target!(|data: (u64, u64, u64)| {
    let (collateral_amount, principal_amount, in_flight_amount) = data;
    let (collateral_amount, principal_amount, in_flight_amount) = (
        u128::from(collateral_amount),
        u128::from(principal_amount),
        u128::from(in_flight_amount),
    );

    let mut position = BorrowPosition::new(0);
    position.collateral_asset_deposit = CollateralAssetAmount::new(collateral_amount);
    position.borrow_asset_principal = BorrowAssetAmount::new(principal_amount);
    position.borrow_asset_in_flight = BorrowAssetAmount::new(in_flight_amount);

    // `get_borrow_asset_principal` is documented to return principal + in_flight.
    // A buggy implementation could drop one of the addends; this assert catches it.
    let getter_principal = position.get_borrow_asset_principal();
    assert_eq!(
        getter_principal,
        position.borrow_asset_principal + position.borrow_asset_in_flight,
        "get_borrow_asset_principal must equal principal + in_flight",
    );

    // Total liability includes principal + in_flight + interest + fees. With
    // interest/fees zero (fresh position), liability must equal principal +
    // in_flight exactly — a buggy implementation that double-counted or
    // dropped a term would fail.
    let liability = position.get_total_borrow_asset_liability();
    assert_eq!(
        liability,
        position.borrow_asset_principal + position.borrow_asset_in_flight,
        "fresh-position liability must equal principal + in_flight exactly",
    );

    // `exists()` is a tight bi-implication on the contributing fields.
    let expected_exists = !position.collateral_asset_deposit.is_zero()
        || !liability.is_zero()
        || !position.collateral_asset_in_flight.is_zero();
    assert_eq!(position.exists(), expected_exists);

    // Clone equality.
    let cloned = position.clone();
    assert_eq!(position, cloned);
});
