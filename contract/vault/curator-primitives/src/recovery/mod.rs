//! Recovery logic for handling failed or stuck operations.
//!
//! This module provides pure functions for determining and executing recovery
//! actions when vault operations fail or get stuck in unexpected states.
//!
//! # Recovery Actions
//!
//! - `KernelAction::AbortAllocating`: Cancel an allocation operation and return to Idle
//! - `KernelAction::AbortWithdrawing`: Cancel a withdrawal operation and refund escrow
//! - `KernelAction::AbortRefreshing`: Cancel a refresh operation and return to Idle
//! - `KernelAction::SettlePayout`: Complete a payout operation (success or failure path)
//!
//! # Design Principles
//!
//! 1. Recovery is deterministic based on state and provided timing context
//! 2. All recovery paths ensure escrow shares are properly handled
//! 3. Recovery should be safe to retry

use alloc::string::String;
use templar_vault_kernel::{
    settle_proportional, AllocatingState, EscrowEntry, EscrowSettlement, KernelAction, OpState,
    PayoutOutcome, PayoutState, RefreshingState, WithdrawingState,
};
use typed_builder::TypedBuilder;

/// Context for determining recovery actions.
#[templar_vault_macros::vault_derive]
#[derive(Clone, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct RecoveryContext {
    /// Current timestamp in nanoseconds.
    pub current_ns: u64,
    /// Maximum time an operation can be in progress before considered stuck.
    /// A value of `0` means "no delay" (treat as immediately eligible).
    #[builder(default)]
    pub stuck_threshold_ns: u64,
    /// Whether to force recovery even if not stuck.
    #[builder(default)]
    pub force_recovery: bool,
}

impl RecoveryContext {
    pub fn new(current_ns: u64) -> Self {
        Self {
            current_ns,
            stuck_threshold_ns: 0,
            force_recovery: false,
        }
    }

    pub fn with_stuck_threshold(current_ns: u64, stuck_threshold_ns: u64) -> Self {
        Self {
            current_ns,
            stuck_threshold_ns,
            force_recovery: false,
        }
    }

    pub fn forced(current_ns: u64) -> Self {
        Self {
            current_ns,
            stuck_threshold_ns: 0,
            force_recovery: true,
        }
    }
}

impl Default for RecoveryContext {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Progress timestamps for an in-flight operation.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct RecoveryProgress {
    /// Timestamp when the operation started.
    pub started_at_ns: u64,
    /// Timestamp of the last forward progress (may equal started_at_ns).
    pub last_progress_ns: u64,
}

impl RecoveryProgress {
    pub const fn new(started_at_ns: u64) -> Self {
        Self {
            started_at_ns,
            last_progress_ns: started_at_ns,
        }
    }

    pub const fn with_last_progress(started_at_ns: u64, last_progress_ns: u64) -> Self {
        Self {
            started_at_ns,
            last_progress_ns,
        }
    }
}

/// Outcome of a recovery operation.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub action: KernelAction,
    pub success: bool,
    pub message: Option<String>,
}

impl RecoveryOutcome {
    pub fn success(action: KernelAction) -> Self {
        Self {
            action,
            success: true,
            message: None,
        }
    }

    pub fn success_with_message(action: KernelAction, message: impl Into<String>) -> Self {
        Self {
            action,
            success: true,
            message: Some(message.into()),
        }
    }

    pub fn failure(action: KernelAction, message: impl Into<String>) -> Self {
        Self {
            action,
            success: false,
            message: Some(message.into()),
        }
    }
}

/// Determine the appropriate recovery action for the current state.
pub fn determine_recovery_action(
    state: &OpState,
    context: &RecoveryContext,
    progress: &RecoveryProgress,
) -> Option<KernelAction> {
    if matches!(state, OpState::Idle) {
        return None;
    }

    if !is_recovery_eligible(context, progress) {
        return None;
    }

    match state {
        OpState::Allocating(alloc) => Some(abort_allocating_action(alloc)),
        OpState::Withdrawing(withdraw) => Some(abort_withdrawing_action(withdraw)),
        OpState::Refreshing(refresh) => Some(abort_refreshing_action(refresh)),
        OpState::Payout(payout) => Some(settle_payout_failure_action(payout, payout.amount)),
        OpState::Idle => None,
    }
}

/// Handle a failed allocation operation.
pub fn handle_allocation_failure(
    state: &AllocatingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    recovery_success_with_message(abort_allocating_action(state), failure_reason)
}

/// Handle a failed withdrawal operation.
pub fn handle_withdrawal_failure(
    state: &WithdrawingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    recovery_success_with_message(abort_withdrawing_action(state), failure_reason)
}

/// Handle a failed refresh operation.
pub fn handle_refresh_failure(
    state: &RefreshingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    recovery_success_with_message(abort_refreshing_action(state), failure_reason)
}

/// Handle a failed payout operation.
pub fn handle_payout_failure(
    state: &PayoutState,
    restore_idle: u128,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    recovery_success_with_message(
        settle_payout_failure_action(state, restore_idle),
        failure_reason,
    )
}

/// Handle a failed payout operation using the payout amount as the idle restore value.
pub fn handle_payout_failure_default(
    state: &PayoutState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    handle_payout_failure(state, state.amount, failure_reason)
}

/// Compute the shares to burn and refund based on collected vs expected amounts.
///
/// If the full withdrawal amount was collected, burn all escrow shares.
/// If partial, compute proportionally.
pub fn compute_settlement_shares(
    escrow_shares: u128,
    expected_amount: u128,
    collected_amount: u128,
) -> EscrowSettlement {
    if expected_amount == 0 || escrow_shares == 0 {
        return EscrowSettlement::refund_all(escrow_shares);
    }

    if collected_amount >= expected_amount {
        return EscrowSettlement::burn_all(escrow_shares);
    }

    settle_proportional(
        &EscrowEntry::new([0u8; 32], escrow_shares, 0, expected_amount),
        collected_amount,
    )
}

/// Compute a success payout outcome from escrow and collected amounts.
///
/// This maps recovery math into kernel `PayoutOutcome::Success`.
#[must_use]
pub fn compute_payout_success_outcome(
    escrow_shares: u128,
    expected_amount: u128,
    collected_amount: u128,
) -> PayoutOutcome {
    let EscrowSettlement {
        to_burn: burn_shares,
        refund: refund_shares,
    } = compute_settlement_shares(escrow_shares, expected_amount, collected_amount);

    PayoutOutcome::Success {
        burn_shares,
        refund_shares,
    }
}

/// Compute a failure payout outcome from escrow shares and idle restore amount.
#[must_use]
pub fn compute_payout_failure_outcome(escrow_shares: u128, restore_idle: u128) -> PayoutOutcome {
    PayoutOutcome::Failure {
        restore_idle,
        refund_shares: escrow_shares,
    }
}

/// Compute recovery statistics from the current state.
///
/// Provides useful metrics for monitoring and debugging recovery operations.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, Default)]
pub struct RecoveryStats {
    /// Number of targets completed before failure (for Allocating/Refreshing).
    pub completed_targets: usize,
    /// Number of targets remaining (for Allocating/Refreshing).
    pub remaining_targets: usize,
    /// Amount already collected (for Withdrawing).
    pub collected_amount: u128,
    /// Amount still needed (for Withdrawing).
    pub remaining_amount: u128,
    /// Shares at risk (for Withdrawing/Payout).
    pub escrow_shares: u128,
}

/// Compute recovery statistics from the current state.
pub fn compute_recovery_stats(state: &OpState) -> RecoveryStats {
    match state {
        OpState::Idle => RecoveryStats::default(),

        OpState::Allocating(alloc) => {
            let completed_targets = (alloc.index as usize).min(alloc.plan.len());
            RecoveryStats {
                completed_targets,
                remaining_targets: alloc.plan.len().saturating_sub(completed_targets),
                remaining_amount: alloc.remaining,
                ..RecoveryStats::default()
            }
        }

        OpState::Withdrawing(withdraw) => RecoveryStats {
            completed_targets: withdraw.index as usize,
            collected_amount: withdraw.collected,
            remaining_amount: withdraw.remaining,
            escrow_shares: withdraw.escrow_shares,
            ..RecoveryStats::default()
        },

        OpState::Refreshing(refresh) => {
            let completed_targets = (refresh.index as usize).min(refresh.plan.len());
            RecoveryStats {
                completed_targets,
                remaining_targets: refresh.plan.len().saturating_sub(completed_targets),
                ..RecoveryStats::default()
            }
        }

        OpState::Payout(payout) => RecoveryStats {
            collected_amount: payout.amount,
            escrow_shares: payout.escrow_shares,
            ..RecoveryStats::default()
        },
    }
}

fn recovery_success_with_message(
    action: KernelAction,
    message: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::success_with_message(action, message)
}

fn is_recovery_eligible(context: &RecoveryContext, progress: &RecoveryProgress) -> bool {
    if context.force_recovery {
        return true;
    }

    let threshold = context.stuck_threshold_ns;
    if threshold == 0 {
        return true;
    }

    context.current_ns.saturating_sub(progress.last_progress_ns) >= threshold
}

fn abort_allocating_action(state: &AllocatingState) -> KernelAction {
    KernelAction::AbortAllocating {
        op_id: state.op_id,
        restore_idle: state.remaining,
    }
}

fn abort_withdrawing_action(state: &WithdrawingState) -> KernelAction {
    KernelAction::AbortWithdrawing {
        op_id: state.op_id,
        refund_shares: state.escrow_shares,
    }
}

fn abort_refreshing_action(state: &RefreshingState) -> KernelAction {
    KernelAction::AbortRefreshing { op_id: state.op_id }
}

fn settle_payout_failure_action(state: &PayoutState, restore_idle: u128) -> KernelAction {
    KernelAction::SettlePayout {
        op_id: state.op_id,
        outcome: compute_payout_failure_outcome(state.escrow_shares, restore_idle),
    }
}
