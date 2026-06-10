//! Recovery planning for failed or stuck vault operations.
//!
//! This module only derives recovery actions from explicit state and evidence.
//! It does not execute those actions, and it does not invent payout outcomes when
//! the available information is incomplete.

use alloc::string::String;
use templar_vault_kernel::{
    settle_proportional_raw, AllocatingState, EscrowSettlement, KernelAction, OpState,
    PayoutOutcome, PayoutState, RefreshingState, WithdrawingState,
};
use typed_builder::TypedBuilder;

/// Recovery eligibility policy.
#[templar_vault_macros::vault_derive]
#[derive(Clone, PartialEq, Eq)]
pub enum RecoveryPolicy {
    /// Never plan recovery automatically.
    Disabled,
    /// Plan recovery after a period of inactivity, with an optional total-age cap.
    AfterInactivity {
        inactivity_threshold_ns: u64,
        max_total_age_ns: Option<u64>,
    },
    /// Plan recovery immediately regardless of progress timestamps.
    Force,
}

/// Context for determining whether recovery is eligible.
#[templar_vault_macros::vault_derive]
#[derive(Clone, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct RecoveryContext {
    /// Current timestamp in nanoseconds.
    pub current_ns: u64,
    /// Recovery policy to apply.
    #[builder(default = RecoveryPolicy::Disabled)]
    pub policy: RecoveryPolicy,
}

impl RecoveryContext {
    #[must_use]
    pub fn new(current_ns: u64) -> Self {
        Self {
            current_ns,
            policy: RecoveryPolicy::Disabled,
        }
    }

    #[must_use]
    pub fn after_inactivity(current_ns: u64, inactivity_threshold_ns: u64) -> Self {
        Self {
            current_ns,
            policy: RecoveryPolicy::AfterInactivity {
                inactivity_threshold_ns,
                max_total_age_ns: None,
            },
        }
    }

    #[must_use]
    pub fn after_inactivity_with_max_age(
        current_ns: u64,
        inactivity_threshold_ns: u64,
        max_total_age_ns: u64,
    ) -> Self {
        Self {
            current_ns,
            policy: RecoveryPolicy::AfterInactivity {
                inactivity_threshold_ns,
                max_total_age_ns: Some(max_total_age_ns),
            },
        }
    }

    #[must_use]
    pub fn forced(current_ns: u64) -> Self {
        Self {
            current_ns,
            policy: RecoveryPolicy::Force,
        }
    }
}

impl Default for RecoveryContext {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Progress timestamps for a specific in-flight operation.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq, TypedBuilder)]
#[builder(field_defaults(setter(into)))]
pub struct RecoveryProgress {
    /// Operation id that these progress timestamps belong to.
    pub op_id: u64,
    /// Timestamp when the operation started.
    pub started_at_ns: u64,
    /// Timestamp of the last forward progress (may equal started_at_ns).
    pub last_progress_ns: u64,
}

impl RecoveryProgress {
    #[must_use]
    pub const fn new(op_id: u64, started_at_ns: u64) -> Self {
        Self {
            op_id,
            started_at_ns,
            last_progress_ns: started_at_ns,
        }
    }

    #[must_use]
    pub const fn with_last_progress(op_id: u64, started_at_ns: u64, last_progress_ns: u64) -> Self {
        Self {
            op_id,
            started_at_ns,
            last_progress_ns,
        }
    }
}

/// Evidence required to settle a payout during recovery.
#[templar_vault_macros::vault_derive]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PayoutRecoveryEvidence {
    /// The payout transfer failed and the provided idle amount must be restored.
    Failure { restore_idle: u128 },
    /// The payout transfer succeeded for the provided collected amount.
    Success { collected_amount: u128 },
}

/// Recovery planning error.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub enum RecoveryError {
    UnknownPayoutState {
        op_id: u64,
    },
    ProgressOpMismatch {
        expected_op_id: u64,
        progress_op_id: u64,
    },
    InvalidProgressTimestamps {
        started_at_ns: u64,
        last_progress_ns: u64,
        current_ns: u64,
    },
    ExpectedAmountZero {
        escrow_shares: u128,
        collected_amount: u128,
    },
    CollectedExceedsExpected {
        expected_amount: u128,
        collected_amount: u128,
    },
    InvalidPayoutEvidence,
}

/// Result of planning a recovery operation.
#[templar_vault_macros::vault_derive(borsh, serde)]
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub action: KernelAction,
    pub planned: bool,
    pub message: Option<String>,
}

impl RecoveryOutcome {
    #[must_use]
    pub fn planned(action: KernelAction) -> Self {
        Self {
            action,
            planned: true,
            message: None,
        }
    }

    #[must_use]
    pub fn planned_with_message(action: KernelAction, message: impl Into<String>) -> Self {
        Self {
            action,
            planned: true,
            message: Some(message.into()),
        }
    }
}

/// Determine the appropriate recovery action for the current state.
pub fn determine_recovery_action(
    state: &OpState,
    context: &RecoveryContext,
    progress: &RecoveryProgress,
    payout_evidence: Option<PayoutRecoveryEvidence>,
) -> Result<Option<KernelAction>, RecoveryError> {
    let Some(op_id) = state_op_id(state) else {
        return Ok(None);
    };

    validate_progress(op_id, context.current_ns, progress)?;

    if !is_recovery_eligible(context, progress) {
        return Ok(None);
    }

    match state {
        OpState::Allocating(alloc) => Ok(Some(abort_allocating_action(alloc))),
        OpState::Withdrawing(withdraw) => Ok(Some(abort_withdrawing_action(withdraw))),
        OpState::Refreshing(refresh) => Ok(Some(abort_refreshing_action(refresh))),
        OpState::Payout(payout) => payout_evidence
            .ok_or(RecoveryError::UnknownPayoutState {
                op_id: payout.op_id,
            })
            .and_then(|evidence| settle_payout_action(payout, evidence).map(Some)),
        OpState::Idle => Ok(None),
    }
}

/// Plan recovery for a failed allocation operation.
#[must_use]
pub fn plan_allocation_recovery(
    state: &AllocatingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::planned_with_message(abort_allocating_action(state), failure_reason)
}

/// Plan recovery for a failed withdrawal operation.
#[must_use]
pub fn plan_withdrawal_recovery(
    state: &WithdrawingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::planned_with_message(abort_withdrawing_action(state), failure_reason)
}

/// Plan recovery for a failed refresh operation.
#[must_use]
pub fn plan_refresh_recovery(
    state: &RefreshingState,
    failure_reason: impl Into<String>,
) -> RecoveryOutcome {
    RecoveryOutcome::planned_with_message(abort_refreshing_action(state), failure_reason)
}

/// Plan recovery for a payout using explicit outcome evidence.
pub fn plan_payout_recovery(
    state: &PayoutState,
    evidence: PayoutRecoveryEvidence,
    failure_reason: impl Into<String>,
) -> Result<RecoveryOutcome, RecoveryError> {
    settle_payout_action(state, evidence)
        .map(|action| RecoveryOutcome::planned_with_message(action, failure_reason))
}

/// Compute the shares to burn and refund based on collected vs expected amounts.
pub fn compute_settlement_shares(
    escrow_shares: u128,
    expected_amount: u128,
    collected_amount: u128,
) -> Result<EscrowSettlement, RecoveryError> {
    if escrow_shares == 0 {
        return Ok(EscrowSettlement::refund_all(0));
    }

    if expected_amount == 0 {
        return Err(RecoveryError::ExpectedAmountZero {
            escrow_shares,
            collected_amount,
        });
    }

    if collected_amount > expected_amount {
        return Err(RecoveryError::CollectedExceedsExpected {
            expected_amount,
            collected_amount,
        });
    }

    Ok(settle_proportional_raw(
        escrow_shares,
        expected_amount,
        collected_amount,
    ))
}

/// Compute a success payout outcome from escrow and collected amounts.
pub fn compute_payout_success_outcome(
    escrow_shares: u128,
    expected_amount: u128,
    collected_amount: u128,
) -> Result<PayoutOutcome, RecoveryError> {
    compute_settlement_shares(escrow_shares, expected_amount, collected_amount)?;
    if collected_amount != expected_amount {
        return Err(RecoveryError::InvalidPayoutEvidence);
    }
    Ok(PayoutOutcome::Success)
}

/// Compute a failure payout outcome from escrow shares and idle restore amount.
pub fn compute_payout_failure_outcome(
    escrow_shares: u128,
    payout_amount: u128,
    restore_idle: u128,
) -> Result<PayoutOutcome, RecoveryError> {
    let _ = escrow_shares;
    if restore_idle != 0 && restore_idle != payout_amount {
        return Err(RecoveryError::InvalidPayoutEvidence);
    }
    Ok(PayoutOutcome::Failure)
}

/// Compute recovery statistics from the current state.
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

fn validate_progress(
    expected_op_id: u64,
    current_ns: u64,
    progress: &RecoveryProgress,
) -> Result<(), RecoveryError> {
    if progress.op_id != expected_op_id {
        return Err(RecoveryError::ProgressOpMismatch {
            expected_op_id,
            progress_op_id: progress.op_id,
        });
    }

    if progress.started_at_ns > progress.last_progress_ns || progress.last_progress_ns > current_ns
    {
        return Err(RecoveryError::InvalidProgressTimestamps {
            started_at_ns: progress.started_at_ns,
            last_progress_ns: progress.last_progress_ns,
            current_ns,
        });
    }

    Ok(())
}

fn is_recovery_eligible(context: &RecoveryContext, progress: &RecoveryProgress) -> bool {
    match &context.policy {
        RecoveryPolicy::Disabled => false,
        RecoveryPolicy::Force => true,
        RecoveryPolicy::AfterInactivity {
            inactivity_threshold_ns,
            max_total_age_ns,
        } => {
            let inactive_for_ns = context.current_ns.saturating_sub(progress.last_progress_ns);
            let total_age_ns = context.current_ns.saturating_sub(progress.started_at_ns);

            inactive_for_ns >= *inactivity_threshold_ns
                || max_total_age_ns.is_some_and(|max_total_age_ns| total_age_ns >= max_total_age_ns)
        }
    }
}

fn state_op_id(state: &OpState) -> Option<u64> {
    match state {
        OpState::Idle => None,
        OpState::Allocating(state) => Some(state.op_id),
        OpState::Withdrawing(state) => Some(state.op_id),
        OpState::Refreshing(state) => Some(state.op_id),
        OpState::Payout(state) => Some(state.op_id),
    }
}

fn abort_allocating_action(state: &AllocatingState) -> KernelAction {
    KernelAction::AbortAllocating { op_id: state.op_id }
}

fn abort_withdrawing_action(state: &WithdrawingState) -> KernelAction {
    KernelAction::AbortWithdrawing { op_id: state.op_id }
}

fn abort_refreshing_action(state: &RefreshingState) -> KernelAction {
    KernelAction::AbortRefreshing { op_id: state.op_id }
}

fn settle_payout_action(
    state: &PayoutState,
    evidence: PayoutRecoveryEvidence,
) -> Result<KernelAction, RecoveryError> {
    let outcome = match evidence {
        PayoutRecoveryEvidence::Failure { restore_idle } => {
            compute_payout_failure_outcome(state.escrow_shares, state.amount, restore_idle)?
        }
        PayoutRecoveryEvidence::Success { collected_amount } => {
            compute_payout_success_outcome(state.escrow_shares, state.amount, collected_amount)?
        }
    };

    Ok(KernelAction::SettlePayout {
        op_id: state.op_id,
        outcome,
    })
}
