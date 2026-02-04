//! Core vault state and configuration types.
//!
//! This module provides the chain-agnostic `VaultState` struct that holds
//! all state required by the kernel, including the withdrawal queue.
//! Executors are responsible for persisting this state to storage.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::fee::FeesSpec;
use crate::state::op_state::OpState;
use crate::state::queue::WithdrawQueue;
use crate::types::TimestampNs;

// ============================================================================
// Constants
// ============================================================================

/// Maximum pending withdrawal queue length.
/// This is an absolute upper bound enforced by the kernel.
pub const MAX_PENDING: usize = 1024;

// ============================================================================
// Fee Anchor
// ============================================================================

/// Anchor point for fee accrual calculations.
///
/// Stores the total assets and timestamp at which fees were last accrued.
/// Used to calculate time-weighted management fees and performance fees
/// based on AUM growth.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeAccrualAnchor {
    /// Total assets at last fee accrual.
    pub total_assets: u128,
    /// Timestamp (nanoseconds) of last fee accrual.
    pub timestamp_ns: TimestampNs,
}

impl FeeAccrualAnchor {
    /// Create a new fee anchor at the given timestamp with the given total assets.
    #[inline]
    #[must_use]
    pub const fn new(total_assets: u128, timestamp_ns: TimestampNs) -> Self {
        Self {
            total_assets,
            timestamp_ns,
        }
    }

    /// Create a fee anchor at time zero with zero assets.
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            total_assets: 0,
            timestamp_ns: 0,
        }
    }

    /// Update the anchor to a new point in time.
    #[inline]
    pub fn update(&mut self, total_assets: u128, timestamp_ns: TimestampNs) {
        self.total_assets = total_assets;
        self.timestamp_ns = timestamp_ns;
    }
}

impl Default for FeeAccrualAnchor {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================================
// Vault Configuration
// ============================================================================

/// Static configuration for a vault.
///
/// These settings can typically only be changed through governance.
///
/// # Fee Recipients
///
/// Fee recipients are 32-byte addresses. Executors are responsible for mapping
/// chain-native account identifiers (e.g., NEAR AccountId, Soroban Address) to
/// this canonical 32-byte format, typically using a SHA256 hash.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultConfig {
    /// Fee configuration (performance, management, growth cap).
    ///
    /// Uses spec-compliant `FeesSpec` with 32-byte address recipients.
    pub fees: FeesSpec,
    /// Minimum withdrawal amount in base asset units.
    pub min_withdrawal_assets: u128,
    /// Maximum number of pending withdrawals allowed in the queue.
    pub max_pending_withdrawals: u32,
    /// Whether the vault is paused (deposits/withdrawals disabled).
    pub paused: bool,
    /// Virtual shares for initial price anchoring.
    /// Added to total_shares when computing share price.
    pub virtual_shares: u128,
    /// Virtual assets for initial price anchoring.
    /// Added to total_assets when computing share price.
    pub virtual_assets: u128,
}

impl VaultConfig {
    /// Check if the max pending withdrawals setting is within bounds.
    /// Enforced by `apply_action` to avoid silent clamping.
    #[inline]
    #[must_use]
    pub fn is_max_pending_valid(&self) -> bool {
        (self.max_pending_withdrawals as usize) <= MAX_PENDING
    }
}

// ============================================================================
// Vault State
// ============================================================================

/// Core kernel vault state.
///
/// This struct contains all the state managed by the kernel. Chain-specific
/// executors are responsible for:
/// - Persisting this state to storage
/// - Handling share/asset token balances
///
/// # Invariants
///
/// - `total_assets == idle_assets + external_assets`
/// - `withdraw_queue.check_invariants()`
/// - `next_op_id` is monotonically increasing
/// - Operations can only proceed when `op_state` allows them
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultState {
    /// Total assets under management (idle + external).
    pub total_assets: u128,
    /// Total vault shares in circulation.
    pub total_shares: u128,
    /// Assets held idle in the vault (not deployed to markets).
    pub idle_assets: u128,
    /// Assets deployed to external markets/strategies.
    pub external_assets: u128,
    /// Anchor for fee accrual calculations.
    pub fee_anchor: FeeAccrualAnchor,
    /// Current operation state machine state.
    pub op_state: OpState,
    /// Pending withdrawal queue owned by the kernel.
    pub withdraw_queue: WithdrawQueue,
    /// Next operation ID to allocate.
    /// Monotonically increasing, never decremented.
    pub next_op_id: u64,
}

impl VaultState {
    /// Create a new vault state initialized to zero/idle.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            total_assets: 0,
            total_shares: 0,
            idle_assets: 0,
            external_assets: 0,
            fee_anchor: FeeAccrualAnchor::zero(),
            op_state: OpState::Idle,
            withdraw_queue: WithdrawQueue::new(),
            next_op_id: 0,
        }
    }

    /// Create a vault state with initial values.
    #[inline]
    #[must_use]
    pub fn with_initial(
        total_assets: u128,
        total_shares: u128,
        idle_assets: u128,
        external_assets: u128,
        timestamp_ns: TimestampNs,
    ) -> Self {
        debug_assert_eq!(
            total_assets,
            idle_assets.saturating_add(external_assets),
            "total_assets invariant violated: total != idle + external",
        );
        Self {
            total_assets,
            total_shares,
            idle_assets,
            external_assets,
            fee_anchor: FeeAccrualAnchor::new(total_assets, timestamp_ns),
            op_state: OpState::Idle,
            withdraw_queue: WithdrawQueue::new(),
            next_op_id: 0,
        }
    }

    /// Check the fundamental accounting invariant.
    ///
    /// Returns `true` if `total_assets == idle_assets + external_assets`.
    #[inline]
    #[must_use]
    pub fn check_invariant(&self) -> bool {
        self.total_assets == self.idle_assets.saturating_add(self.external_assets)
            && self.withdraw_queue.check_invariants()
    }

    /// Allocate and return the next operation ID.
    ///
    /// Increments `next_op_id` and returns the previous value.
    #[inline]
    pub fn allocate_op_id(&mut self) -> u64 {
        let id = self.next_op_id;
        self.next_op_id = self.next_op_id.saturating_add(1);
        id
    }

    /// Check if the vault is idle (no operation in progress).
    #[inline]
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.op_state.is_idle()
    }

    /// Get the current operation ID if an operation is in progress.
    #[inline]
    #[must_use]
    pub fn current_op_id(&self) -> Option<u64> {
        self.op_state.op_id()
    }
}

impl Default for VaultState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
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

        let config = VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 1000,
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
}
