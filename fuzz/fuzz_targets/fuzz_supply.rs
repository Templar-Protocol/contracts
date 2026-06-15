//! Fuzz `SupplyPosition` / `Deposit` real functions.
//!
//! Rule 7: `SupplyPosition::can_be_removed` still exists (the prior disable
//! TODO claimed it was removed — it was not; verified against
//! `common/src/supply.rs`). It is exercised here.
//!
//! Rule 3: deposit amounts are bounded to `u64` so the
//! `active + outgoing + Σ incoming` sum in `Deposit::total()` cannot trip the
//! contract's intentional `u128` overflow abort. That boundary is fuzzed by
//! `fuzz_supply_overflow` (the P2 backstop) and asserted by the unit test
//! `supply::tests::deposit_total_overflow_aborts`.

#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use libfuzzer_sys::fuzz_target;
use templar_common::{
    asset::BorrowAssetAmount,
    incoming_deposit::IncomingDeposit,
    supply::{Deposit, SupplyPosition},
};

// MUTATION-CHECK (P5): in `Deposit::total` (supply.rs:29), drop the
// `+ self.outgoing` term (or skip an `incoming` entry). Then the
// `total == active + outgoing + Σ incoming` assertion below must fire.

fuzz_target!(|data: (u32, u64, u64, u64, u32, u32, u64)| {
    let (
        snapshot_index,
        active_amount,
        incoming_amount_1,
        incoming_amount_2,
        activate_at_1,
        activate_at_2,
        outgoing_amount,
    ) = data;

    // ---- SupplyPosition basic invariants (P2) ----
    let fresh = SupplyPosition::new(snapshot_index);
    // exists() and can_be_removed() must be exact complements, always.
    assert_eq!(
        fresh.exists(),
        !fresh.can_be_removed(),
        "exists() and can_be_removed() must be complements",
    );
    // A fresh position is empty.
    assert!(!fresh.exists(), "fresh SupplyPosition must not exist");
    assert!(
        fresh.can_be_removed(),
        "fresh SupplyPosition must be removable"
    );

    // ---- Deposit::total / total_incoming (P2) ----
    let mut incoming = Vec::new();
    if incoming_amount_1 > 0 && activate_at_1 > snapshot_index {
        incoming.push(IncomingDeposit {
            activate_at_snapshot_index: activate_at_1,
            amount: BorrowAssetAmount::new(u128::from(incoming_amount_1)),
        });
    }
    if incoming_amount_2 > 0 && activate_at_2 > snapshot_index && activate_at_2 != activate_at_1 {
        incoming.push(IncomingDeposit {
            activate_at_snapshot_index: activate_at_2,
            amount: BorrowAssetAmount::new(u128::from(incoming_amount_2)),
        });
    }

    let deposit = Deposit {
        active: BorrowAssetAmount::new(u128::from(active_amount)),
        incoming: incoming.clone(),
        outgoing: BorrowAssetAmount::new(u128::from(outgoing_amount)),
    };

    // total() must equal active + outgoing + Σ incoming, exactly. A buggy
    // implementation that dropped `outgoing` or an incoming entry, or
    // double-counted, would fail this.
    let incoming_sum: u128 = incoming.iter().map(|i| u128::from(i.amount)).sum();
    let expected_total = u128::from(active_amount) + u128::from(outgoing_amount) + incoming_sum;
    assert_eq!(
        u128::from(deposit.total()),
        expected_total,
        "Deposit::total must equal active + outgoing + Σ incoming",
    );

    // Adding more incoming must never decrease total() (monotonicity a buggy
    // accumulation could violate).
    let mut deposit_more = deposit.clone();
    deposit_more.incoming.push(IncomingDeposit {
        activate_at_snapshot_index: snapshot_index.wrapping_add(100),
        amount: BorrowAssetAmount::new(1),
    });
    assert!(
        deposit_more.total() >= deposit.total(),
        "adding an incoming deposit must not decrease total()",
    );
});
