use super::*;
use crate::state::queue::{PendingWithdrawal, WithdrawQueue};
use alloc::collections::BTreeMap;

#[test]
fn test_fee_anchor_new() {
    let anchor = FeeAccrualAnchor::new(1000, 123456789);
    assert_eq!(anchor.total_assets, 1000);
    assert_eq!(anchor.timestamp_ns, 123456789);
}

#[test]
fn test_fee_anchor_zero() {
    let anchor = FeeAccrualAnchor::zero();
    assert_eq!(anchor.total_assets, 0);
    assert_eq!(anchor.timestamp_ns, 0);
}

#[test]
fn test_fee_anchor_update() {
    let mut anchor = FeeAccrualAnchor::zero();
    anchor.update(5000, 999);
    assert_eq!(anchor.total_assets, 5000);
    assert_eq!(anchor.timestamp_ns, 999);
}

#[test]
fn test_vault_state_new() {
    let state = VaultState::new();
    assert_eq!(state.total_assets, 0);
    assert_eq!(state.total_shares, 0);
    assert_eq!(state.idle_assets, 0);
    assert_eq!(state.external_assets, 0);
    assert_eq!(state.next_op_id, 0);
    assert!(state.is_idle());
    assert!(state.check_invariant());
}

#[test]
fn test_vault_state_with_initial() {
    let state = VaultState::with_initial(1000, 500, 400, 600, 123);
    assert_eq!(state.total_assets, 1000);
    assert_eq!(state.total_shares, 500);
    assert_eq!(state.idle_assets, 400);
    assert_eq!(state.external_assets, 600);
    assert_eq!(state.fee_anchor.total_assets, 1000);
    assert_eq!(state.fee_anchor.timestamp_ns, 123);
    assert!(state.is_idle());
    assert!(state.check_invariant());
}

#[test]
fn test_vault_state_invariant_violation() {
    let mut state = VaultState::new();
    state.total_assets = 1000;
    state.idle_assets = 400;
    state.external_assets = 500; // 400 + 500 = 900 != 1000
    assert!(!state.check_invariant());
}

#[test]
fn test_vault_state_queue_invariant_violation() {
    let mut pending = BTreeMap::new();
    pending.insert(
        5,
        PendingWithdrawal::new([1u8; 32], [1u8; 32], 100, 1000, 0),
    );

    let mut state = VaultState::new();
    state.withdraw_queue = WithdrawQueue::with_state(pending, 0, 6);
    assert!(!state.check_invariant());
}

#[test]
fn test_allocate_op_id() {
    let mut state = VaultState::new();
    assert_eq!(state.allocate_op_id(), 0);
    assert_eq!(state.allocate_op_id(), 1);
    assert_eq!(state.allocate_op_id(), 2);
    assert_eq!(state.next_op_id, 3);
}

#[test]
fn test_allocate_op_id_saturating() {
    let mut state = VaultState::new();
    state.next_op_id = u64::MAX;
    assert_eq!(state.allocate_op_id(), u64::MAX);
    assert_eq!(state.next_op_id, u64::MAX); // saturates
}

#[test]
fn test_vault_state_default() {
    let state = VaultState::default();
    assert_eq!(state.total_assets, 0);
    assert!(state.is_idle());
}

#[test]
fn test_vault_config_max_pending_valid() {
    use crate::fee::FeesSpec;
    use crate::state::queue::DEFAULT_COOLDOWN_NS;

    let config = VaultConfig {
        fees: FeesSpec::zero(),
        min_withdrawal_assets: 1000,
        withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
        max_pending_withdrawals: 1024,
        paused: false,
        virtual_shares: 0,
        virtual_assets: 0,
    };
    assert!(config.is_max_pending_valid());

    let config_invalid = VaultConfig {
        max_pending_withdrawals: 2000,
        ..config
    };
    assert!(!config_invalid.is_max_pending_valid());
}
