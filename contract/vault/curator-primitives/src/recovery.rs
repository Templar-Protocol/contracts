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
//! 1. Recovery is deterministic based on current state
//! 2. All recovery paths ensure escrow shares are properly handled
//! 3. Recovery should be safe to retry

use alloc::string::String;
use templar_vault_kernel::{
    AllocatingState, EscrowSettlement, KernelAction, OpState, PayoutOutcome, PayoutState,
    RefreshingState, WithdrawingState,
};

/// Context for determining recovery actions.
#[derive(Clone, Debug)]
pub struct RecoveryContext {
    /// Current timestamp in nanoseconds.
    pub current_ns: u64,
    /// Maximum time an operation can be in progress before considered stuck.
    pub stuck_threshold_ns: u64,
    /// Whether to force recovery even if not stuck.
    pub force_recovery: bool,
}

impl RecoveryContext {
    /// Create a new recovery context.
    pub fn new(current_ns: u64) -> Self {
        Self {
            current_ns,
            stuck_threshold_ns: 0,
            force_recovery: false,
        }
    }

    /// Create a context with a stuck threshold.
    pub fn with_stuck_threshold(current_ns: u64, stuck_threshold_ns: u64) -> Self {
        Self {
            current_ns,
            stuck_threshold_ns,
            force_recovery: false,
        }
    }

    /// Create a context that forces recovery.
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

/// Errors that can occur during recovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    /// Cannot recover from this state.
    InvalidState { state_name: &'static str },
    /// Operation is not stuck (and force not set).
    NotStuck { op_id: u64 },
    /// Recovery action conflicts with current state.
    ActionConflict {
        action: &'static str,
        state: &'static str,
    },
}

/// Outcome of a recovery operation.
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryOutcome {
    /// The action that was taken.
    pub action: KernelAction,
    /// Whether recovery was successful.
    pub success: bool,
    /// Optional message describing the outcome.
    pub message: Option<String>,
}

impl RecoveryOutcome {
    /// Create a successful recovery outcome.
    pub fn success(action: KernelAction) -> Self {
        Self {
            action,
            success: true,
            message: None,
        }
    }

    /// Create a successful outcome with a message.
    pub fn success_with_message(action: KernelAction, message: impl Into<String>) -> Self {
        Self {
            action,
            success: true,
            message: Some(message.into()),
        }
    }

    /// Create a failed recovery outcome.
    pub fn failure(action: KernelAction, message: impl Into<String>) -> Self {
        Self {
            action,
            success: false,
            message: Some(message.into()),
        }
    }
}

/// Determine the appropriate recovery action for the current state.
///
/// # Arguments
/// * `state` - The current operation state
/// * `context` - Recovery context with timestamps and settings
///
/// # Returns
/// The recovery action to take, if any.
pub fn determine_recovery_action(
    state: &OpState,
    _context: &RecoveryContext,
) -> Option<KernelAction> {
    match state {
        OpState::Idle => None,
        OpState::Allocating(alloc) => Some(KernelAction::AbortAllocating {
            op_id: alloc.op_id,
            restore_idle: alloc.remaining,
        }),
        OpState::Withdrawing(withdraw) => Some(KernelAction::AbortWithdrawing {
            op_id: withdraw.op_id,
            refund_shares: withdraw.escrow_shares,
        }),
        OpState::Refreshing(refresh) => Some(KernelAction::AbortRefreshing {
            op_id: refresh.op_id,
        }),
        OpState::Payout(payout) => Some(KernelAction::SettlePayout {
            op_id: payout.op_id,
            outcome: compute_payout_failure_outcome(payout.escrow_shares, payout.amount),
        }),
    }
}

/// Handle a failed allocation operation.
///
/// # Arguments
/// * `state` - The allocating state
/// * `failure_reason` - Description of why the allocation failed
///
/// # Returns
/// Recovery outcome with the abort action.
pub fn handle_allocation_failure(
    state: &AllocatingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::success_with_message(
        KernelAction::AbortAllocating {
            op_id: state.op_id,
            restore_idle: state.remaining,
        },
        failure_reason,
    )
}

/// Handle a failed withdrawal operation.
///
/// # Arguments
/// * `state` - The withdrawing state
/// * `failure_reason` - Description of why the withdrawal failed
///
/// # Returns
/// Recovery outcome with escrow refund.
pub fn handle_withdrawal_failure(
    state: &WithdrawingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::success_with_message(
        KernelAction::AbortWithdrawing {
            op_id: state.op_id,
            refund_shares: state.escrow_shares,
        },
        failure_reason,
    )
}

/// Handle a failed refresh operation.
///
/// # Arguments
/// * `state` - The refreshing state
/// * `failure_reason` - Description of why the refresh failed
///
/// # Returns
/// Recovery outcome with completed/remaining target info.
pub fn handle_refresh_failure(
    state: &RefreshingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::success_with_message(
        KernelAction::AbortRefreshing { op_id: state.op_id },
        failure_reason,
    )
}

/// Handle a failed payout operation.
///
/// # Arguments
/// * `state` - The payout state
/// * `failure_reason` - Description of why the payout failed
///
/// # Returns
/// Recovery outcome with full escrow refund.
pub fn handle_payout_failure(
    state: &PayoutState,
    restore_idle: u128,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::success_with_message(
        KernelAction::SettlePayout {
            op_id: state.op_id,
            outcome: compute_payout_failure_outcome(state.escrow_shares, restore_idle),
        },
        failure_reason,
    )
}

/// Compute the shares to burn and refund based on collected vs expected amounts.
///
/// If the full withdrawal amount was collected, burn all escrow shares.
/// If partial, compute proportionally.
///
/// # Arguments
/// * `escrow_shares` - Total shares in escrow
/// * `expected_amount` - Expected withdrawal amount
/// * `collected_amount` - Actually collected amount
///
/// # Returns
/// Escrow settlement with burn/refund amounts.
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

    let burn = escrow_shares
        .saturating_mul(collected_amount)
        .checked_div(expected_amount)
        .unwrap_or(0);

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
#[derive(Clone, Debug, Default)]
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
///
/// # Arguments
/// * `state` - The current operation state
///
/// # Returns
/// Recovery statistics for the state.
pub fn compute_recovery_stats(state: &OpState) -> RecoveryStats {
    match state {
        OpState::Idle => RecoveryStats::default(),

        OpState::Allocating(alloc) => RecoveryStats {
            completed_targets: alloc.index as usize,
            remaining_targets: alloc.plan.len().saturating_sub(alloc.index as usize),
            collected_amount: 0,
            remaining_amount: alloc.remaining,
            escrow_shares: 0,
        },

        OpState::Withdrawing(withdraw) => RecoveryStats {
            completed_targets: withdraw.index as usize,
            remaining_targets: 0, // Unknown without route info
            collected_amount: withdraw.collected,
            remaining_amount: withdraw.remaining,
            escrow_shares: withdraw.escrow_shares,
        },

        OpState::Refreshing(refresh) => RecoveryStats {
            completed_targets: refresh.index as usize,
            remaining_targets: refresh.plan.len().saturating_sub(refresh.index as usize),
            collected_amount: 0,
            remaining_amount: 0,
            escrow_shares: 0,
        },

        OpState::Payout(payout) => RecoveryStats {
            completed_targets: 0,
            remaining_targets: 0,
            collected_amount: payout.amount,
            remaining_amount: 0,
            escrow_shares: payout.escrow_shares,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use templar_vault_kernel::Address;

    fn addr_with_tag(tag: u8, index: u64) -> Address {
        let mut addr = [0u8; 32];
        addr[0] = tag;
        addr[1..9].copy_from_slice(&index.to_le_bytes());
        addr
    }

    fn owner_addr(index: u64) -> Address {
        addr_with_tag(0x11, index)
    }

    fn receiver_addr(index: u64) -> Address {
        addr_with_tag(0x22, index)
    }

    #[test]
    fn test_determine_recovery_action_idle() {
        let state = OpState::Idle;
        let ctx = RecoveryContext::new(1000);

        let action = determine_recovery_action(&state, &ctx);

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

        let action = determine_recovery_action(&state, &ctx).expect("expected action");

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

        let action = determine_recovery_action(&state, &ctx).expect("expected action");

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

        let action = determine_recovery_action(&state, &ctx).expect("expected action");

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

        let action = determine_recovery_action(&state, &ctx).expect("expected action");

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
