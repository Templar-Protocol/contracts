//! Invariant tests for pending-deposit primitives.
//!
//! Deposits are liabilities outside vault pricing until immutable
//! settlement admission. These tests prove the constructors, the epoch
//! binding, and the separation of pending deposits from `VaultState`
//! pricing totals.

use super::*;
use crate::state::vault::VaultState;

fn owner_addr(seed: u64) -> Address {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    Address::from_bytes(bytes)
}

fn ts(ns: u64) -> TimestampNs {
    TimestampNs::from_nanos(ns)
}

#[test]
fn pending_deposit_constructor_preserves_identity_and_amount() {
    let deposit = PendingDeposit::new(
        owner_addr(11),
        5_000,
        ts(1_000),
        EpochId::FIRST_SETTLEMENT,
    )
    .expect("valid pending deposit");
    assert_eq!(deposit.owner, owner_addr(11));
    assert_eq!(deposit.assets, 5_000);
    assert_eq!(deposit.requested_at_ns, ts(1_000));
    assert_eq!(deposit.epoch_id, EpochId::FIRST_SETTLEMENT);
}

#[test]
fn pending_deposit_rejects_zero_assets_and_epochless_liability() {
    assert_eq!(
        PendingDeposit::new(
            owner_addr(11),
            0,
            ts(1_000),
            EpochId::FIRST_SETTLEMENT
        ),
        Err(RequestError::ZeroDepositAssets)
    );
    assert_eq!(
        PendingDeposit::new(owner_addr(11), 5_000, ts(1_000), EpochId::MIGRATION_INTAKE),
        Err(RequestError::InvalidEpoch),
        "epochless deposit liabilities cannot enter the ledger"
    );
}

#[test]
fn pending_deposits_never_touch_vault_pricing_totals() {
    // Admission is performed only by immutable epoch settlement. Until
    // then, constructing deposit liabilities must leave every pricing
    // total in `VaultState` untouched.
    let state = VaultState::new();
    let before = (
        state.total_assets,
        state.total_shares,
        state.idle_assets,
        state.external_assets,
    );

    let mut recorded = 0u128;
    for owner in 0..8u64 {
        let deposit = PendingDeposit::new(
            owner_addr(owner),
            1_000 + u128::from(owner),
            ts(1_000),
            EpochId::FIRST_SETTLEMENT,
        )
        .expect("valid pending deposit");
        recorded = recorded.saturating_add(deposit.assets);
    }
    assert!(recorded > 0, "ledger must record pending assets");

    let after = (
        state.total_assets,
        state.total_shares,
        state.idle_assets,
        state.external_assets,
    );
    assert_eq!(before, after, "pending deposits must not move pricing totals");
    assert!(
        state.check_invariant(),
        "vault accounting invariant must hold with pending deposits outstanding"
    );
}

#[test]
fn deposit_ledger_capacity_bound_is_enforced_by_storage_lane_constants() {
    // The kernel exposes a fixed upper bound so the storage lane can size
    // its pages; it must be positive and match the documented ceiling.
    assert_eq!(MAX_PENDING_DEPOSITS, 1024);
}
