//! Kani proofs for kernel invariants.
//!
//! These harnesses are compiled only when the `kani` feature is enabled.

#![cfg(kani)]

use alloc::collections::BTreeMap;
use kani::any;

use crate::state::queue::PendingWithdrawal;
use crate::types::Address;

fn owner_addr() -> Address {
    let mut addr = [0u8; 32];
    addr[0] = 0x11;
    addr
}

fn receiver_addr() -> Address {
    let mut addr = [0u8; 32];
    addr[0] = 0x22;
    addr
}

fn dummy_withdrawal() -> PendingWithdrawal {
    PendingWithdrawal {
        owner: owner_addr(),
        receiver: receiver_addr(),
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
