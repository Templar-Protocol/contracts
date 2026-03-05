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

/// Maximum pending withdrawal queue length.
/// This is an absolute upper bound enforced by the kernel.
pub const MAX_PENDING: usize = 1024;

/// Anchor point for fee accrual calculations.
///
/// Stores the total assets and timestamp at which fees were last accrued.
/// Used to calculate time-weighted management fees and performance fees
/// based on AUM growth.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FeeAccrualAnchor {
    pub total_assets: u128,
    pub timestamp_ns: TimestampNs,
}

impl FeeAccrualAnchor {
    #[inline]
    #[must_use]
    pub const fn new(total_assets: u128, timestamp_ns: TimestampNs) -> Self {
        Self {
            total_assets,
            timestamp_ns,
        }
    }

    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            total_assets: 0,
            timestamp_ns: 0,
        }
    }

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
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VaultConfig {
    pub fees: FeesSpec,
    pub min_withdrawal_assets: u128,
    pub withdrawal_cooldown_ns: u64,
    pub max_pending_withdrawals: u32,
    pub paused: bool,
    pub virtual_shares: u128,
    pub virtual_assets: u128,
}

impl VaultConfig {
    #[inline]
    #[must_use]
    pub fn is_max_pending_valid(&self) -> bool {
        (self.max_pending_withdrawals as usize) <= MAX_PENDING
    }
}

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
#[cfg_attr(
    all(feature = "postcard", not(feature = "serde")),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct VaultState {
    pub total_assets: u128,
    pub total_shares: u128,
    pub idle_assets: u128,
    pub external_assets: u128,
    pub fee_anchor: FeeAccrualAnchor,
    pub op_state: OpState,
    pub withdraw_queue: WithdrawQueue,
    pub next_op_id: u64,
}

impl VaultState {
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

    /// Recompute `total_assets` from `idle_assets + external_assets`.
    ///
    /// Call this after any mutation of `idle_assets` or `external_assets`
    /// to restore the fundamental accounting invariant.
    #[inline]
    pub fn sync_total_assets(&mut self) {
        self.total_assets = self.idle_assets.saturating_add(self.external_assets);
    }

    /// Add `amount` back to idle assets and recompute totals.
    ///
    /// Common pattern during abort / emergency-reset paths where
    /// in-flight assets are returned to idle.
    #[inline]
    pub fn restore_to_idle(&mut self, amount: u128) {
        self.idle_assets = self.idle_assets.saturating_add(amount);
        self.sync_total_assets();
    }
}

impl Default for VaultState {
    fn default() -> Self {
        Self::new()
    }
}

// Tests

#[cfg(test)]
mod tests;
