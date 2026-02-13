//! Operation state machine for asynchronous vault operations.
//!
//! This module provides a chain-agnostic state machine for managing the lifecycle
//! of allocation, withdrawal, refresh, and payout operations in a vault.
//!
//! # State Machine
//!
//! ```text
//!                    +-------+
//!                    | Idle  |<-----------------------+
//!                    +-------+                        |
//!                        |                            |
//!          +-------------+-------------+              |
//!          |                           |              |
//!          v                           v              |
//!    +------------+            +-------------+        |
//!    | Allocating |            | Refreshing  |--------+
//!    +------------+            +-------------+        |
//!          |                                          |
//!          | (on completion or stop)                  |
//!          v                                          |
//!    +-------------+                                  |
//!    | Withdrawing |----------------------------------+
//!    +-------------+                                  |
//!          |                                          |
//!          | (when enough collected)                  |
//!          v                                          |
//!    +--------+                                       |
//!    | Payout |---------------------------------------+
//!    +--------+
//! ```
//!
use alloc::vec::Vec;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use derive_more::{From, IsVariant};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::types::Address;

pub type TargetId = u32;

/// No operation in-flight. The vault is ready to start a new allocation or withdrawal.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdleState;

/// Supplying idle underlying to targets according to a plan or queue.
///
/// # Transitions
/// - On completion of allocation: `Withdrawing` (to satisfy pending user requests) or `Idle` (if stopped).
/// - On stop/failure: `Idle`.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocatingState {
    pub op_id: u64,
    pub index: u32,
    pub remaining: u128,
    pub plan: Vec<(TargetId, u128)>,
}

/// Collecting liquidity from targets to satisfy a user withdrawal/redeem request.
///
/// # Transitions
/// - Advance within queue: `Withdrawing` (index increments) while collecting funds.
/// - When enough is collected to satisfy the request: `Payout`.
/// - If the op is stopped or cannot proceed and needs to refund: `Idle` (escrow_shares refunded).
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawingState {
    pub op_id: u64,
    pub index: u32,
    pub remaining: u128,
    pub collected: u128,
    pub receiver: Address,
    pub owner: Address,
    pub escrow_shares: u128,
}

/// Read-only refresh of target principals to update stored AUM.
///
/// # Transitions
/// - On completion: `Idle`.
/// - On failure: `Idle` (with potentially stale AUM data).
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshingState {
    pub op_id: u64,
    pub index: u32,
    pub plan: Vec<TargetId>,
}

/// Final step that transfers assets to the receiver and settles the share escrow.
///
/// # Transitions
/// - On success or failure: `Idle`.
///
/// # Invariant hooks
/// - `idle_balance` decreases only on payout success by `amount`.
/// - On success, `burn_shares` are burned from `escrow_shares`; any remainder is refunded.
/// - On failure, all `escrow_shares` are refunded.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutState {
    pub op_id: u64,
    pub receiver: Address,
    pub amount: u128,
    pub owner: Address,
    pub escrow_shares: u128,
    pub burn_shares: u128,
}

impl AllocatingState {
    /// Advance to the next allocation step after `amount_allocated` was supplied.
    #[inline]
    #[must_use]
    pub fn advance(self, amount_allocated: u128) -> Self {
        Self {
            op_id: self.op_id,
            index: self.index.saturating_add(1),
            remaining: self.remaining.saturating_sub(amount_allocated),
            plan: self.plan,
        }
    }
}

impl WithdrawingState {
    /// Advance to the next withdrawal step after `amount_collected` was received.
    #[inline]
    #[must_use]
    pub fn advance(&self, amount_collected: u128) -> Self {
        Self {
            op_id: self.op_id,
            index: self.index.saturating_add(1),
            remaining: self.remaining.saturating_sub(amount_collected),
            collected: self.collected.saturating_add(amount_collected),
            receiver: self.receiver,
            owner: self.owner,
            escrow_shares: self.escrow_shares,
        }
    }
}

impl RefreshingState {
    /// Advance to the next refresh step.
    #[inline]
    #[must_use]
    pub fn advance(self) -> Self {
        Self {
            op_id: self.op_id,
            index: self.index.saturating_add(1),
            plan: self.plan,
        }
    }
}

/// Operation state machine for asynchronous allocation, withdrawal, and payout flows.
///
/// # State Machine
/// - `Allocating` -> `Withdrawing` (or `Idle` via stop)
/// - `Withdrawing` -> `Withdrawing` (advance) | `Payout` | `Idle` (refund)
/// - `Refreshing` -> `Idle`
/// - `Payout` -> `Idle` (success or failure)
///
/// # Invariants
/// - `idle_balance` increases only when funds are received and decreases only on payout success.
/// - `escrow_shares` are refunded on stop/failure or partially burned/refunded on payout success.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Default, PartialEq, Eq, From, IsVariant)]
pub enum OpState {
    /// No operation in-flight. The vault is ready to start a new allocation or withdrawal.
    #[default]
    Idle,

    /// Supplying idle underlying to targets according to a plan or queue.
    ///
    /// # Transitions
    /// - On completion of allocation: `Withdrawing` (to satisfy pending user requests) or `Idle` (if stopped).
    /// - On stop/failure: `Idle`.
    Allocating(AllocatingState),

    /// Collecting liquidity from targets to satisfy a user withdrawal/redeem request.
    ///
    /// # Transitions
    /// - Advance within queue: `Withdrawing` (index increments) while collecting funds.
    /// - When enough is collected to satisfy the request: `Payout`.
    /// - If the op is stopped or cannot proceed and needs to refund: `Idle` (escrow_shares refunded).
    Withdrawing(WithdrawingState),

    /// Read-only refresh of target principals to update stored AUM.
    Refreshing(RefreshingState),

    /// Final step that transfers assets to the receiver and settles the share escrow.
    ///
    /// # Transitions
    /// - On success or failure: `Idle`.
    ///
    /// # Invariant hooks
    /// - `idle_balance` decreases only on payout success by `amount`.
    /// - On success, `burn_shares` are burned from `escrow_shares`; any remainder is refunded.
    /// - On failure, all `escrow_shares` are refunded.
    Payout(PayoutState),
}

// Note: From<AllocatingState>, From<WithdrawingState>, From<RefreshingState>,
// From<PayoutState> are auto-generated by derive_more::From

impl From<IdleState> for OpState {
    fn from(_: IdleState) -> Self {
        OpState::Idle
    }
}

// --- Accessor methods ---

impl OpState {
    /// Returns a numeric code for the current op state.
    #[inline]
    #[must_use]
    pub const fn kind_code(&self) -> u32 {
        match self {
            OpState::Idle => 0,
            OpState::Allocating(_) => 1,
            OpState::Withdrawing(_) => 2,
            OpState::Refreshing(_) => 3,
            OpState::Payout(_) => 4,
        }
    }

    /// Returns a human-readable name for the current op state.
    #[inline]
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            OpState::Idle => "Idle",
            OpState::Allocating(_) => "Allocating",
            OpState::Withdrawing(_) => "Withdrawing",
            OpState::Refreshing(_) => "Refreshing",
            OpState::Payout(_) => "Payout",
        }
    }

    /// Returns a reference to the idle state if this is `Idle`, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn as_idle(&self) -> Option<&IdleState> {
        match self {
            OpState::Idle => Some(&IdleState),
            _ => None,
        }
    }

    /// Returns a reference to the allocating state if this is `Allocating`, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn as_allocating(&self) -> Option<&AllocatingState> {
        match self {
            OpState::Allocating(s) => Some(s),
            _ => None,
        }
    }

    /// Returns a reference to the withdrawing state if this is `Withdrawing`, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn as_withdrawing(&self) -> Option<&WithdrawingState> {
        match self {
            OpState::Withdrawing(s) => Some(s),
            _ => None,
        }
    }

    /// Returns a reference to the refreshing state if this is `Refreshing`, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn as_refreshing(&self) -> Option<&RefreshingState> {
        match self {
            OpState::Refreshing(s) => Some(s),
            _ => None,
        }
    }

    /// Returns a reference to the payout state if this is `Payout`, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn as_payout(&self) -> Option<&PayoutState> {
        match self {
            OpState::Payout(s) => Some(s),
            _ => None,
        }
    }

    // Note: is_idle(), is_allocating(), is_withdrawing(), is_refreshing(), is_payout()
    // are auto-generated by derive_more::IsVariant

    /// Returns the operation ID if this state has one, otherwise `None`.
    #[inline]
    #[must_use]
    pub const fn op_id(&self) -> Option<u64> {
        match self {
            OpState::Idle => None,
            OpState::Allocating(s) => Some(s.op_id),
            OpState::Withdrawing(s) => Some(s.op_id),
            OpState::Refreshing(s) => Some(s.op_id),
            OpState::Payout(s) => Some(s.op_id),
        }
    }
}

#[cfg(test)]
mod tests;
