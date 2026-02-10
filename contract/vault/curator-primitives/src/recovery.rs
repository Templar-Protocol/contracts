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
    AllocatingState, EscrowSettlement, KernelAction, OpState, PayoutOutcome, PayoutState,
    RefreshingState, WithdrawingState,
};
use typed_builder::TypedBuilder;

/// Context for determining recovery actions.
#[derive(Clone, Debug, TypedBuilder)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, TypedBuilder)]
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
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
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
        OpState::Idle => None,
        OpState::Allocating(alloc) => Some(abort_allocating_action(alloc)),
        OpState::Withdrawing(withdraw) => Some(abort_withdrawing_action(withdraw)),
        OpState::Refreshing(refresh) => Some(abort_refreshing_action(refresh)),
        OpState::Payout(payout) => Some(settle_payout_failure_action(payout, payout.amount)),
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

    let burn =
        (escrow_shares.saturating_mul(collected_amount) / expected_amount).min(escrow_shares);

    EscrowSettlement::partial(burn, escrow_shares.saturating_sub(burn))
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
#[derive(Clone, Copy, Debug, Default)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{owner_addr, receiver_addr};
    use alloc::string::String;
    use alloc::vec;

    #[test]
    fn test_determine_recovery_action_idle() {
        let state = OpState::Idle;

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress);

        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300), (3, 200)],
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortAllocating {
                op_id,
                restore_idle,
            } => {
                assert_eq!(op_id, 1);
                assert_eq!(restore_idle, 500);
            }
            _ => panic!("Expected AbortAllocating"),
        }
    }

    #[test]
    fn test_determine_recovery_action_not_stuck() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            plan: vec![(0, 100)],
        });

        let ctx = RecoveryContext::with_stuck_threshold(1_000, 500);
        let progress = RecoveryProgress::with_last_progress(900, 900);

        let action = determine_recovery_action(&state, &ctx, &progress);
        assert!(action.is_none());
    }

    #[test]
    fn test_determine_recovery_action_forced_ignores_threshold() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 11,
            index: 0,
            remaining: 100,
            plan: vec![(0, 100)],
        });

        let ctx = RecoveryContext::forced(1_000);
        let progress = RecoveryProgress::with_last_progress(999, 999);

        let action = determine_recovery_action(&state, &ctx, &progress);
        assert!(action.is_some());
    }

    #[test]
    fn test_determine_recovery_action_withdrawing() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortWithdrawing {
                op_id,
                refund_shares,
            } => {
                assert_eq!(op_id, 2);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected AbortWithdrawing"),
        }
    }

    #[test]
    fn test_determine_recovery_action_refreshing() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 3,
            index: 1,
            plan: vec![0, 1, 2],
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_determine_recovery_action_payout() {
        let state = OpState::Payout(PayoutState {
            op_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let ctx = RecoveryContext::new(1000);
        let progress = RecoveryProgress::new(0);

        let action = determine_recovery_action(&state, &ctx, &progress).expect("expected action");

        match action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1000);
                        assert_eq!(refund_shares, 500);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_compute_settlement_shares_full_collection() {
        let settlement = compute_settlement_shares(1000, 500, 500);
        assert_eq!(settlement.to_burn, 1000);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_compute_settlement_shares_partial_collection() {
        let settlement = compute_settlement_shares(1000, 500, 250);
        // burn = 1000 * 250 / 500 = 500
        assert_eq!(settlement.to_burn, 500);
        assert_eq!(settlement.refund, 500);
    }

    #[test]
    fn test_compute_settlement_shares_over_collection() {
        // Collected more than expected (edge case)
        let settlement = compute_settlement_shares(1000, 500, 600);
        assert_eq!(settlement.to_burn, 1000);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_compute_payout_success_outcome_maps_settlement() {
        let outcome = compute_payout_success_outcome(1000, 500, 250);
        match outcome {
            PayoutOutcome::Success {
                burn_shares,
                refund_shares,
            } => {
                assert_eq!(burn_shares, 500);
                assert_eq!(refund_shares, 500);
            }
            _ => panic!("Expected success outcome"),
        }
    }

    #[test]
    fn test_compute_payout_failure_outcome_refunds_all() {
        let outcome = compute_payout_failure_outcome(1000, 250);
        match outcome {
            PayoutOutcome::Failure {
                restore_idle,
                refund_shares,
            } => {
                assert_eq!(restore_idle, 250);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected failure outcome"),
        }
    }

    #[test]
    fn test_compute_settlement_shares_zero_expected() {
        let settlement = compute_settlement_shares(1000, 0, 0);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 1000);
    }

    #[test]
    fn test_compute_settlement_shares_zero_escrow() {
        let settlement = compute_settlement_shares(0, 500, 250);
        assert_eq!(settlement.to_burn, 0);
        assert_eq!(settlement.refund, 0);
    }

    #[test]
    fn test_handle_allocation_failure() {
        let state = AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300)],
        };

        let outcome = handle_allocation_failure(&state, "Market unavailable");

        assert!(outcome.success);
        assert_eq!(outcome.message, Some(String::from("Market unavailable")));
        match outcome.action {
            KernelAction::AbortAllocating {
                op_id,
                restore_idle,
            } => {
                assert_eq!(op_id, 1);
                assert_eq!(restore_idle, 500);
            }
            _ => panic!("Expected AbortAllocating"),
        }
    }

    #[test]
    fn test_handle_withdrawal_failure() {
        let state = WithdrawingState {
            op_id: 2,
            index: 1,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        };

        let outcome = handle_withdrawal_failure(&state, "Insufficient liquidity");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::AbortWithdrawing {
                op_id,
                refund_shares,
            } => {
                assert_eq!(op_id, 2);
                assert_eq!(refund_shares, 1000);
            }
            _ => panic!("Expected AbortWithdrawing"),
        }
    }

    #[test]
    fn test_handle_refresh_failure() {
        let state = RefreshingState {
            op_id: 3,
            index: 1,
            plan: vec![0, 1, 2],
        };

        let outcome = handle_refresh_failure(&state, "Oracle unavailable");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::AbortRefreshing { op_id } => {
                assert_eq!(op_id, 3);
            }
            _ => panic!("Expected AbortRefreshing"),
        }
    }

    #[test]
    fn test_handle_payout_failure() {
        let state = PayoutState {
            op_id: 4,
            receiver: receiver_addr(1),
            amount: 1000,
            owner: owner_addr(1),
            escrow_shares: 500,
            burn_shares: 400,
        };

        let outcome = handle_payout_failure(&state, 1000, "Transfer rejected");

        assert!(outcome.success);
        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 4);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1000);
                        assert_eq!(refund_shares, 500);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_handle_payout_failure_default_uses_amount() {
        let state = PayoutState {
            op_id: 5,
            receiver: receiver_addr(2),
            amount: 1500,
            owner: owner_addr(2),
            escrow_shares: 750,
            burn_shares: 0,
        };

        let outcome = handle_payout_failure_default(&state, "Transfer rejected");

        match outcome.action {
            KernelAction::SettlePayout { op_id, outcome } => {
                assert_eq!(op_id, 5);
                match outcome {
                    PayoutOutcome::Failure {
                        restore_idle,
                        refund_shares,
                    } => {
                        assert_eq!(restore_idle, 1500);
                        assert_eq!(refund_shares, 750);
                    }
                    _ => panic!("Expected failure outcome"),
                }
            }
            _ => panic!("Expected SettlePayout"),
        }
    }

    #[test]
    fn test_compute_recovery_stats_allocating() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 500,
            plan: vec![(0, 300), (1, 200), (2, 300), (3, 200)],
        });

        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 2);
        assert_eq!(stats.remaining_targets, 2);
        assert_eq!(stats.remaining_amount, 500);
        assert_eq!(stats.escrow_shares, 0);
    }

    #[test]
    fn test_compute_recovery_stats_withdrawing() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 2,
            index: 3,
            remaining: 400,
            collected: 600,
            receiver: receiver_addr(1),
            owner: owner_addr(1),
            escrow_shares: 1000,
        });

        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 3);
        assert_eq!(stats.collected_amount, 600);
        assert_eq!(stats.remaining_amount, 400);
        assert_eq!(stats.escrow_shares, 1000);
    }

    #[test]
    fn test_compute_recovery_stats_idle() {
        let state = OpState::Idle;
        let stats = compute_recovery_stats(&state);

        assert_eq!(stats.completed_targets, 0);
        assert_eq!(stats.remaining_targets, 0);
        assert_eq!(stats.collected_amount, 0);
        assert_eq!(stats.remaining_amount, 0);
        assert_eq!(stats.escrow_shares, 0);
    }

    #[test]
    fn test_recovery_outcome_creation() {
        let action = KernelAction::AbortRefreshing { op_id: 1 };

        let success = RecoveryOutcome::success(action.clone());
        assert!(success.success);
        assert!(success.message.is_none());

        let with_msg = RecoveryOutcome::success_with_message(action.clone(), "All good");
        assert!(with_msg.success);
        assert_eq!(with_msg.message, Some(String::from("All good")));

        let failure = RecoveryOutcome::failure(action, "Something went wrong");
        assert!(!failure.success);
        assert_eq!(failure.message, Some(String::from("Something went wrong")));
    }
}
