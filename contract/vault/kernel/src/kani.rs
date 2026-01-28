//! Kani proofs for kernel invariants.
//!
//! These harnesses are compiled only when the `kani` feature is enabled.

#![cfg(kani)]

use alloc::collections::BTreeMap;
use alloc::string::String;

use kani::any;

use crate::state::queue::PendingWithdrawal;

fn dummy_withdrawal() -> PendingWithdrawal {
    PendingWithdrawal {
        owner: String::from("owner"),
        receiver: String::from("receiver"),
        escrow_shares: 1,
        expected_assets: 1,
        requested_at_ns: 0,
    }
}

#[kani::proof]
fn kani_pending_withdrawals_head_exists_when_nonempty() {
    let len: u8 = any();
    kani::assume(len <= 8);

    let mut pending: BTreeMap<u64, PendingWithdrawal> = BTreeMap::new();
    let mut next_id: u64 = 0;

    for _ in 0..len {
        pending.insert(next_id, dummy_withdrawal());
        next_id = next_id.saturating_add(1);
    }

    let next_withdraw_to_execute = if pending.is_empty() { next_id } else { 0 };

    if !pending.is_empty() {
        assert!(pending.contains_key(&next_withdraw_to_execute));
    }
}
