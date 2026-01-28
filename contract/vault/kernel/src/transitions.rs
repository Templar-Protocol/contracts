//! Pure transition functions for the OpState machine.
//!
//! These functions define how the vault's operation state machine changes state
//! in response to events. They are pure functions: no side effects, no storage access.
//!
//! # Design Principles
//!
//! 1. **Pure Functions**: Each transition takes the current state and inputs,
//!    returning a new state and a list of effects to execute.
//! 2. **Explicit State Requirements**: Transitions check that the machine is in
//!    the expected state before proceeding.
//! 3. **Effect-Based Output**: Side effects (transfers, burns, etc.) are returned
//!    as `KernelEffect` values rather than executed directly.
//!
//! # State Machine
//!
//! ```text
//! Idle -> Allocating (start_allocation)
//! Idle -> Withdrawing (start_withdrawal)
//! Idle -> Refreshing (start_refresh)
//! Allocating -> Withdrawing | Idle (complete_allocation)
//! Withdrawing -> Withdrawing (advance_withdrawal)
//! Withdrawing -> Payout (withdrawal_collected)
//! Withdrawing -> Idle (stop_withdrawal)
//! Refreshing -> Idle (complete_refresh)
//! Payout -> Idle (payout_complete)
//! ```

use alloc::vec;
use alloc::vec::Vec;

use crate::effects::{KernelEffect, KernelEvent};
use crate::state::op_state::{
    AllocatingState, OpState, PayoutState, RefreshingState, TargetId, WithdrawingState,
};
use crate::types::ActorId;

/// Error types for state transitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionError {
    /// Attempted a transition that requires Idle state, but the machine was not Idle.
    NotIdle { current_state: &'static str },
    /// Attempted a transition that requires Allocating state.
    NotAllocating { current_state: &'static str },
    /// Attempted a transition that requires Withdrawing state.
    NotWithdrawing { current_state: &'static str },
    /// Attempted a transition that requires Refreshing state.
    NotRefreshing { current_state: &'static str },
    /// Attempted a transition that requires Payout state.
    NotPayout { current_state: &'static str },
    /// Operation ID mismatch - the callback doesn't match the current operation.
    OpIdMismatch { expected: u64, actual: u64 },
    /// The allocation plan is empty.
    EmptyAllocationPlan,
    /// The refresh plan is empty.
    EmptyRefreshPlan,
    /// Zero amount requested for withdrawal.
    ZeroWithdrawalAmount,
    /// Zero escrow shares - nothing to withdraw.
    ZeroEscrowShares,
    /// Invalid index in the operation.
    InvalidIndex { index: u32, max: u32 },
    /// Attempted to collect more than remaining.
    CollectionOverflow { collected: u128, remaining: u128 },
    /// Burn shares exceed escrow shares.
    BurnExceedsEscrow { burn: u128, escrow: u128 },
}

impl TransitionError {
    fn state_name(state: &OpState) -> &'static str {
        match state {
            OpState::Idle => "Idle",
            OpState::Allocating(_) => "Allocating",
            OpState::Withdrawing(_) => "Withdrawing",
            OpState::Refreshing(_) => "Refreshing",
            OpState::Payout(_) => "Payout",
        }
    }
}

/// Result of a successful state transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionResult {
    /// The new state after the transition.
    pub new_state: OpState,
    /// Effects to execute as a result of this transition.
    pub effects: Vec<KernelEffect>,
}

impl TransitionResult {
    /// Create a transition result with no effects.
    pub fn new(new_state: OpState) -> Self {
        Self {
            new_state,
            effects: vec![],
        }
    }

    /// Create a transition result with effects.
    pub fn with_effects(new_state: OpState, effects: Vec<KernelEffect>) -> Self {
        Self { new_state, effects }
    }
}

/// Type alias for transition function results.
pub type TransitionRes = Result<TransitionResult, TransitionError>;

// =============================================================================
// Allocation Transitions
// =============================================================================

/// Start an allocation from Idle state.
///
/// # Arguments
/// * `state` - Current state (must be Idle)
/// * `plan` - List of (target_id, amount) pairs specifying where to allocate
/// * `op_id` - Unique operation ID for correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with new Allocating state
/// * `Err(TransitionError::NotIdle)` if not in Idle state
/// * `Err(TransitionError::EmptyAllocationPlan)` if plan is empty
pub fn start_allocation(state: OpState, plan: Vec<(TargetId, u128)>, op_id: u64) -> TransitionRes {
    if !state.is_idle() {
        return Err(TransitionError::NotIdle {
            current_state: TransitionError::state_name(&state),
        });
    }

    if plan.is_empty() {
        return Err(TransitionError::EmptyAllocationPlan);
    }

    let total: u128 = plan.iter().map(|(_, amt)| amt).sum();

    let new_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: total,
        plan,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::Placeholder,
        }],
    ))
}

/// Process one step of allocation (callback from market).
///
/// Advances the allocation index and updates remaining amount.
///
/// # Arguments
/// * `state` - Current state (must be Allocating)
/// * `success` - Whether the allocation step succeeded
/// * `amount_allocated` - Amount that was actually allocated in this step
/// * `op_id` - Operation ID to verify correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with updated Allocating state
/// * `Err` on state mismatch or op_id mismatch
pub fn allocation_step_callback(
    state: OpState,
    success: bool,
    amount_allocated: u128,
    op_id: u64,
) -> TransitionRes {
    let alloc = match &state {
        OpState::Allocating(s) => s,
        _ => {
            return Err(TransitionError::NotAllocating {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if alloc.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: alloc.op_id,
            actual: op_id,
        });
    }

    if !success {
        // On failure, return to Idle
        return Ok(TransitionResult::with_effects(
            OpState::Idle,
            vec![KernelEffect::EmitEvent {
                event: KernelEvent::Placeholder,
            }],
        ));
    }

    let new_remaining = alloc.remaining.saturating_sub(amount_allocated);
    let new_index = alloc.index + 1;

    let new_state = OpState::Allocating(AllocatingState {
        op_id: alloc.op_id,
        index: new_index,
        remaining: new_remaining,
        plan: alloc.plan.clone(),
    });

    Ok(TransitionResult::new(new_state))
}

/// Complete allocation and transition to next state.
///
/// # Arguments
/// * `state` - Current state (must be Allocating)
/// * `op_id` - Operation ID to verify correlation
/// * `pending_withdrawal` - Optional pending withdrawal to process next
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle or Withdrawing state
pub fn complete_allocation(
    state: OpState,
    op_id: u64,
    pending_withdrawal: Option<WithdrawalRequest>,
) -> TransitionRes {
    let alloc = match &state {
        OpState::Allocating(s) => s,
        _ => {
            return Err(TransitionError::NotAllocating {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if alloc.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: alloc.op_id,
            actual: op_id,
        });
    }

    match pending_withdrawal {
        Some(req) => {
            // Transition to Withdrawing to process the pending request
            let new_state = OpState::Withdrawing(WithdrawingState {
                op_id: req.op_id,
                index: 0,
                remaining: req.amount,
                collected: 0,
                receiver: req.receiver,
                owner: req.owner,
                escrow_shares: req.escrow_shares,
            });
            Ok(TransitionResult::with_effects(
                new_state,
                vec![KernelEffect::EmitEvent {
                    event: KernelEvent::Placeholder,
                }],
            ))
        }
        None => {
            // No pending withdrawal, return to Idle
            Ok(TransitionResult::with_effects(
                OpState::Idle,
                vec![KernelEffect::EmitEvent {
                    event: KernelEvent::Placeholder,
                }],
            ))
        }
    }
}

// =============================================================================
// Withdrawal Transitions
// =============================================================================

/// Request for a withdrawal operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawalRequest {
    /// Unique operation ID.
    pub op_id: u64,
    /// Amount of assets to withdraw.
    pub amount: u128,
    /// Receiver of the assets.
    pub receiver: ActorId,
    /// Owner of the shares being redeemed.
    pub owner: ActorId,
    /// Shares held in escrow for this withdrawal.
    pub escrow_shares: u128,
}

/// Start a withdrawal from Idle state.
///
/// # Arguments
/// * `state` - Current state (must be Idle)
/// * `request` - Withdrawal request details
///
/// # Returns
/// * `Ok(TransitionResult)` with new Withdrawing state
/// * `Err` on validation failure
pub fn start_withdrawal(state: OpState, request: WithdrawalRequest) -> TransitionRes {
    if !state.is_idle() {
        return Err(TransitionError::NotIdle {
            current_state: TransitionError::state_name(&state),
        });
    }

    if request.amount == 0 {
        return Err(TransitionError::ZeroWithdrawalAmount);
    }

    if request.escrow_shares == 0 {
        return Err(TransitionError::ZeroEscrowShares);
    }

    let new_state = OpState::Withdrawing(WithdrawingState {
        op_id: request.op_id,
        index: 0,
        remaining: request.amount,
        collected: 0,
        receiver: request.receiver,
        owner: request.owner,
        escrow_shares: request.escrow_shares,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::Placeholder,
        }],
    ))
}

/// Advance withdrawal by recording collected funds.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `op_id` - Operation ID to verify correlation
/// * `amount_collected` - Amount collected in this step
///
/// # Returns
/// * `Ok(TransitionResult)` with updated Withdrawing state or Payout state
pub fn withdrawal_step_callback(
    state: OpState,
    op_id: u64,
    amount_collected: u128,
) -> TransitionRes {
    let withdraw = match &state {
        OpState::Withdrawing(s) => s,
        _ => {
            return Err(TransitionError::NotWithdrawing {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    let new_collected = withdraw.collected.saturating_add(amount_collected);
    let new_remaining = withdraw.remaining.saturating_sub(amount_collected);
    let new_index = withdraw.index + 1;

    let new_state = OpState::Withdrawing(WithdrawingState {
        op_id: withdraw.op_id,
        index: new_index,
        remaining: new_remaining,
        collected: new_collected,
        receiver: withdraw.receiver.clone(),
        owner: withdraw.owner.clone(),
        escrow_shares: withdraw.escrow_shares,
    });

    Ok(TransitionResult::new(new_state))
}

/// Transition from Withdrawing to Payout when enough has been collected.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `op_id` - Operation ID to verify correlation
/// * `burn_shares` - Number of shares to burn on successful payout
///
/// # Returns
/// * `Ok(TransitionResult)` with Payout state
pub fn withdrawal_collected(state: OpState, op_id: u64, burn_shares: u128) -> TransitionRes {
    let withdraw = match &state {
        OpState::Withdrawing(s) => s,
        _ => {
            return Err(TransitionError::NotWithdrawing {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    if burn_shares > withdraw.escrow_shares {
        return Err(TransitionError::BurnExceedsEscrow {
            burn: burn_shares,
            escrow: withdraw.escrow_shares,
        });
    }

    let new_state = OpState::Payout(PayoutState {
        op_id: withdraw.op_id,
        receiver: withdraw.receiver.clone(),
        amount: withdraw.collected,
        owner: withdraw.owner.clone(),
        escrow_shares: withdraw.escrow_shares,
        burn_shares,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::Placeholder,
        }],
    ))
}

/// Stop withdrawal and refund escrow shares.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `op_id` - Operation ID to verify correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle state and refund effects
pub fn stop_withdrawal(state: OpState, op_id: u64) -> TransitionRes {
    let withdraw = match &state {
        OpState::Withdrawing(s) => s,
        _ => {
            return Err(TransitionError::NotWithdrawing {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    // Refund all escrow shares to owner
    let mut effects = vec![];

    // Transfer shares back from escrow to owner (represented as an effect)
    // The actual escrow address would be handled by the runtime
    if withdraw.escrow_shares > 0 {
        // Using a placeholder address for escrow - runtime will substitute
        let escrow_address = [0u8; 32];
        let mut owner_address = [0u8; 32];
        // Copy owner bytes if available (simplified - real impl would convert)
        let owner_bytes = withdraw.owner.as_bytes();
        let len = owner_bytes.len().min(32);
        owner_address[..len].copy_from_slice(&owner_bytes[..len]);

        effects.push(KernelEffect::TransferShares {
            from: escrow_address,
            to: owner_address,
            shares: withdraw.escrow_shares,
        });
    }

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::Placeholder,
    });

    Ok(TransitionResult::with_effects(OpState::Idle, effects))
}

// =============================================================================
// Refresh Transitions
// =============================================================================

/// Start a refresh operation from Idle state.
///
/// # Arguments
/// * `state` - Current state (must be Idle)
/// * `plan` - List of target IDs to refresh
/// * `op_id` - Unique operation ID
///
/// # Returns
/// * `Ok(TransitionResult)` with new Refreshing state
pub fn start_refresh(state: OpState, plan: Vec<TargetId>, op_id: u64) -> TransitionRes {
    if !state.is_idle() {
        return Err(TransitionError::NotIdle {
            current_state: TransitionError::state_name(&state),
        });
    }

    if plan.is_empty() {
        return Err(TransitionError::EmptyRefreshPlan);
    }

    let new_state = OpState::Refreshing(RefreshingState {
        op_id,
        index: 0,
        plan,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::Placeholder,
        }],
    ))
}

/// Process one step of refresh (callback from target).
///
/// # Arguments
/// * `state` - Current state (must be Refreshing)
/// * `op_id` - Operation ID to verify correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with updated Refreshing state
pub fn refresh_step_callback(state: OpState, op_id: u64) -> TransitionRes {
    let refresh = match &state {
        OpState::Refreshing(s) => s,
        _ => {
            return Err(TransitionError::NotRefreshing {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if refresh.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: refresh.op_id,
            actual: op_id,
        });
    }

    let new_index = refresh.index + 1;

    let new_state = OpState::Refreshing(RefreshingState {
        op_id: refresh.op_id,
        index: new_index,
        plan: refresh.plan.clone(),
    });

    Ok(TransitionResult::new(new_state))
}

/// Complete refresh and return to Idle.
///
/// # Arguments
/// * `state` - Current state (must be Refreshing)
/// * `op_id` - Operation ID to verify correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle state
pub fn complete_refresh(state: OpState, op_id: u64) -> TransitionRes {
    let refresh = match &state {
        OpState::Refreshing(s) => s,
        _ => {
            return Err(TransitionError::NotRefreshing {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if refresh.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: refresh.op_id,
            actual: op_id,
        });
    }

    Ok(TransitionResult::with_effects(
        OpState::Idle,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::Placeholder,
        }],
    ))
}

// =============================================================================
// Payout Transitions
// =============================================================================

/// Complete payout and return to Idle.
///
/// # Arguments
/// * `state` - Current state (must be Payout)
/// * `success` - Whether the transfer succeeded
/// * `op_id` - Operation ID to verify correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle state and appropriate effects
pub fn payout_complete(state: OpState, success: bool, op_id: u64) -> TransitionRes {
    let payout = match &state {
        OpState::Payout(s) => s,
        _ => {
            return Err(TransitionError::NotPayout {
                current_state: TransitionError::state_name(&state),
            });
        }
    };

    if payout.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: payout.op_id,
            actual: op_id,
        });
    }

    let mut effects = vec![];

    // Convert owner to address bytes (simplified)
    let mut owner_address = [0u8; 32];
    let owner_bytes = payout.owner.as_bytes();
    let len = owner_bytes.len().min(32);
    owner_address[..len].copy_from_slice(&owner_bytes[..len]);

    if success {
        // Burn the designated shares
        if payout.burn_shares > 0 {
            effects.push(KernelEffect::BurnShares {
                owner: owner_address,
                shares: payout.burn_shares,
            });
        }

        // Refund any remaining escrow shares
        let refund_shares = payout.escrow_shares.saturating_sub(payout.burn_shares);
        if refund_shares > 0 {
            let escrow_address = [0u8; 32];
            effects.push(KernelEffect::TransferShares {
                from: escrow_address,
                to: owner_address,
                shares: refund_shares,
            });
        }
    } else {
        // On failure, refund all escrow shares
        if payout.escrow_shares > 0 {
            let escrow_address = [0u8; 32];
            effects.push(KernelEffect::TransferShares {
                from: escrow_address,
                to: owner_address,
                shares: payout.escrow_shares,
            });
        }
    }

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::Placeholder,
    });

    Ok(TransitionResult::with_effects(OpState::Idle, effects))
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    // -------------------------------------------------------------------------
    // Allocation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_start_allocation_from_idle() {
        let state = OpState::Idle;
        let plan = vec![(0, 500), (1, 500)];
        let op_id = 1;

        let result = start_allocation(state, plan.clone(), op_id).unwrap();

        assert!(result.new_state.is_allocating());
        let alloc = result.new_state.as_allocating().unwrap();
        assert_eq!(alloc.op_id, op_id);
        assert_eq!(alloc.index, 0);
        assert_eq!(alloc.remaining, 1000);
        assert_eq!(alloc.plan, plan);
    }

    #[test]
    fn test_start_allocation_not_idle_error() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 1,
            index: 0,
            plan: vec![0],
        });
        let plan = vec![(0, 500)];

        let result = start_allocation(state, plan, 2);

        assert!(matches!(result, Err(TransitionError::NotIdle { .. })));
    }

    #[test]
    fn test_start_allocation_empty_plan_error() {
        let state = OpState::Idle;
        let plan = vec![];

        let result = start_allocation(state, plan, 1);

        assert!(matches!(result, Err(TransitionError::EmptyAllocationPlan)));
    }

    #[test]
    fn test_allocation_step_callback_success() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 1000,
            plan: vec![(0, 500), (1, 500)],
        });

        let result = allocation_step_callback(state, true, 500, 1).unwrap();

        let alloc = result.new_state.as_allocating().unwrap();
        assert_eq!(alloc.index, 1);
        assert_eq!(alloc.remaining, 500);
    }

    #[test]
    fn test_allocation_step_callback_failure_returns_idle() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 1000,
            plan: vec![(0, 500)],
        });

        let result = allocation_step_callback(state, false, 0, 1).unwrap();

        assert!(result.new_state.is_idle());
    }

    #[test]
    fn test_allocation_step_callback_wrong_op_id() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 1000,
            plan: vec![(0, 500)],
        });

        let result = allocation_step_callback(state, true, 500, 999);

        assert!(matches!(
            result,
            Err(TransitionError::OpIdMismatch {
                expected: 1,
                actual: 999
            })
        ));
    }

    #[test]
    fn test_complete_allocation_to_idle() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 0,
            plan: vec![(0, 500), (1, 500)],
        });

        let result = complete_allocation(state, 1, None).unwrap();

        assert!(result.new_state.is_idle());
    }

    #[test]
    fn test_complete_allocation_to_withdrawing() {
        let state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 2,
            remaining: 0,
            plan: vec![(0, 500)],
        });

        let request = WithdrawalRequest {
            op_id: 2,
            amount: 300,
            receiver: String::from("receiver.near"),
            owner: String::from("owner.near"),
            escrow_shares: 100,
        };

        let result = complete_allocation(state, 1, Some(request)).unwrap();

        assert!(result.new_state.is_withdrawing());
        let withdraw = result.new_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, 2);
        assert_eq!(withdraw.remaining, 300);
        assert_eq!(withdraw.receiver, "receiver.near");
    }

    // -------------------------------------------------------------------------
    // Withdrawal Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_start_withdrawal_from_idle() {
        let state = OpState::Idle;
        let request = WithdrawalRequest {
            op_id: 1,
            amount: 1000,
            receiver: String::from("receiver.near"),
            owner: String::from("owner.near"),
            escrow_shares: 500,
        };

        let result = start_withdrawal(state, request).unwrap();

        assert!(result.new_state.is_withdrawing());
        let withdraw = result.new_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, 1);
        assert_eq!(withdraw.remaining, 1000);
        assert_eq!(withdraw.collected, 0);
        assert_eq!(withdraw.escrow_shares, 500);
    }

    #[test]
    fn test_start_withdrawal_zero_amount_error() {
        let state = OpState::Idle;
        let request = WithdrawalRequest {
            op_id: 1,
            amount: 0,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 100,
        };

        let result = start_withdrawal(state, request);

        assert!(matches!(result, Err(TransitionError::ZeroWithdrawalAmount)));
    }

    #[test]
    fn test_start_withdrawal_zero_escrow_error() {
        let state = OpState::Idle;
        let request = WithdrawalRequest {
            op_id: 1,
            amount: 1000,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 0,
        };

        let result = start_withdrawal(state, request);

        assert!(matches!(result, Err(TransitionError::ZeroEscrowShares)));
    }

    #[test]
    fn test_withdrawal_step_callback() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 1,
            index: 0,
            remaining: 1000,
            collected: 0,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 500,
        });

        let result = withdrawal_step_callback(state, 1, 400).unwrap();

        let withdraw = result.new_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.index, 1);
        assert_eq!(withdraw.remaining, 600);
        assert_eq!(withdraw.collected, 400);
    }

    #[test]
    fn test_withdrawal_collected_to_payout() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 1,
            index: 2,
            remaining: 0,
            collected: 1000,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 500,
        });

        let result = withdrawal_collected(state, 1, 400).unwrap();

        assert!(result.new_state.is_payout());
        let payout = result.new_state.as_payout().unwrap();
        assert_eq!(payout.amount, 1000);
        assert_eq!(payout.burn_shares, 400);
        assert_eq!(payout.escrow_shares, 500);
    }

    #[test]
    fn test_withdrawal_collected_burn_exceeds_escrow_error() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 1,
            index: 0,
            remaining: 0,
            collected: 1000,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 500,
        });

        let result = withdrawal_collected(state, 1, 600);

        assert!(matches!(
            result,
            Err(TransitionError::BurnExceedsEscrow {
                burn: 600,
                escrow: 500
            })
        ));
    }

    #[test]
    fn test_stop_withdrawal_refunds_shares() {
        let state = OpState::Withdrawing(WithdrawingState {
            op_id: 1,
            index: 1,
            remaining: 500,
            collected: 500,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 100,
        });

        let result = stop_withdrawal(state, 1).unwrap();

        assert!(result.new_state.is_idle());
        // Should have a TransferShares effect for refund
        assert!(result
            .effects
            .iter()
            .any(|e| matches!(e, KernelEffect::TransferShares { shares: 100, .. })));
    }

    // -------------------------------------------------------------------------
    // Refresh Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_start_refresh_from_idle() {
        let state = OpState::Idle;
        let plan = vec![0, 1, 2];

        let result = start_refresh(state, plan.clone(), 1).unwrap();

        assert!(result.new_state.is_refreshing());
        let refresh = result.new_state.as_refreshing().unwrap();
        assert_eq!(refresh.op_id, 1);
        assert_eq!(refresh.index, 0);
        assert_eq!(refresh.plan, plan);
    }

    #[test]
    fn test_start_refresh_empty_plan_error() {
        let state = OpState::Idle;

        let result = start_refresh(state, vec![], 1);

        assert!(matches!(result, Err(TransitionError::EmptyRefreshPlan)));
    }

    #[test]
    fn test_refresh_step_callback() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 1,
            index: 0,
            plan: vec![0, 1],
        });

        let result = refresh_step_callback(state, 1).unwrap();

        let refresh = result.new_state.as_refreshing().unwrap();
        assert_eq!(refresh.index, 1);
    }

    #[test]
    fn test_complete_refresh_to_idle() {
        let state = OpState::Refreshing(RefreshingState {
            op_id: 1,
            index: 2,
            plan: vec![0, 1],
        });

        let result = complete_refresh(state, 1).unwrap();

        assert!(result.new_state.is_idle());
    }

    // -------------------------------------------------------------------------
    // Payout Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_payout_complete_success() {
        let state = OpState::Payout(PayoutState {
            op_id: 1,
            receiver: String::from("receiver"),
            amount: 1000,
            owner: String::from("owner"),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let result = payout_complete(state, true, 1).unwrap();

        assert!(result.new_state.is_idle());

        // Should have BurnShares effect
        assert!(result
            .effects
            .iter()
            .any(|e| matches!(e, KernelEffect::BurnShares { shares: 400, .. })));

        // Should have TransferShares effect for refund (500 - 400 = 100)
        assert!(result
            .effects
            .iter()
            .any(|e| matches!(e, KernelEffect::TransferShares { shares: 100, .. })));
    }

    #[test]
    fn test_payout_complete_failure_refunds_all() {
        let state = OpState::Payout(PayoutState {
            op_id: 1,
            receiver: String::from("receiver"),
            amount: 1000,
            owner: String::from("owner"),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let result = payout_complete(state, false, 1).unwrap();

        assert!(result.new_state.is_idle());

        // Should NOT have BurnShares effect
        assert!(!result
            .effects
            .iter()
            .any(|e| matches!(e, KernelEffect::BurnShares { .. })));

        // Should have TransferShares effect for full refund (500 shares)
        assert!(result
            .effects
            .iter()
            .any(|e| matches!(e, KernelEffect::TransferShares { shares: 500, .. })));
    }

    #[test]
    fn test_payout_complete_wrong_state_error() {
        let state = OpState::Idle;

        let result = payout_complete(state, true, 1);

        assert!(matches!(result, Err(TransitionError::NotPayout { .. })));
    }

    #[test]
    fn test_payout_complete_wrong_op_id_error() {
        let state = OpState::Payout(PayoutState {
            op_id: 1,
            receiver: String::from("receiver"),
            amount: 1000,
            owner: String::from("owner"),
            escrow_shares: 500,
            burn_shares: 400,
        });

        let result = payout_complete(state, true, 999);

        assert!(matches!(
            result,
            Err(TransitionError::OpIdMismatch {
                expected: 1,
                actual: 999
            })
        ));
    }

    // -------------------------------------------------------------------------
    // State Machine Flow Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_full_allocation_flow() {
        // Start from Idle
        let state = OpState::Idle;

        // Start allocation
        let result = start_allocation(state, vec![(0, 500), (1, 500)], 1).unwrap();
        assert!(result.new_state.is_allocating());

        // First step callback
        let result = allocation_step_callback(result.new_state, true, 500, 1).unwrap();
        assert!(result.new_state.is_allocating());

        // Second step callback
        let result = allocation_step_callback(result.new_state, true, 500, 1).unwrap();
        assert!(result.new_state.is_allocating());

        // Complete allocation, no pending withdrawal
        let result = complete_allocation(result.new_state, 1, None).unwrap();
        assert!(result.new_state.is_idle());
    }

    #[test]
    fn test_full_withdrawal_flow() {
        // Start from Idle
        let state = OpState::Idle;

        // Start withdrawal
        let request = WithdrawalRequest {
            op_id: 1,
            amount: 1000,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 500,
        };
        let result = start_withdrawal(state, request).unwrap();
        assert!(result.new_state.is_withdrawing());

        // Collect funds
        let result = withdrawal_step_callback(result.new_state, 1, 500).unwrap();
        assert!(result.new_state.is_withdrawing());

        let result = withdrawal_step_callback(result.new_state, 1, 500).unwrap();
        assert!(result.new_state.is_withdrawing());

        // Transition to payout
        let result = withdrawal_collected(result.new_state, 1, 400).unwrap();
        assert!(result.new_state.is_payout());

        // Complete payout
        let result = payout_complete(result.new_state, true, 1).unwrap();
        assert!(result.new_state.is_idle());
    }

    #[test]
    fn test_allocation_to_withdrawal_flow() {
        // Start allocation
        let state = OpState::Idle;
        let result = start_allocation(state, vec![(0, 1000)], 1).unwrap();

        // Complete step
        let result = allocation_step_callback(result.new_state, true, 1000, 1).unwrap();

        // Complete allocation with pending withdrawal
        let request = WithdrawalRequest {
            op_id: 2,
            amount: 500,
            receiver: String::from("receiver"),
            owner: String::from("owner"),
            escrow_shares: 250,
        };
        let result = complete_allocation(result.new_state, 1, Some(request)).unwrap();
        assert!(result.new_state.is_withdrawing());

        let withdraw = result.new_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, 2);
    }
}

// ============================================================================
// Property Tests for State Transitions
// ============================================================================

#[cfg(test)]
mod proptests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use proptest::prelude::*;

    /// Strategy for generating an allocation plan
    fn arb_plan(max_len: usize) -> impl Strategy<Value = Vec<(TargetId, u128)>> {
        proptest::collection::vec((0u32..100u32, 1u128..=1_000_000_000u128), 1..=max_len)
    }

    /// Strategy for generating a withdrawal request
    fn arb_withdrawal_request() -> impl Strategy<Value = WithdrawalRequest> {
        (
            1u64..u64::MAX,            // op_id
            1u128..=1_000_000_000u128, // amount
            1u128..=1_000_000_000u128, // escrow_shares
        )
            .prop_map(|(op_id, amount, escrow_shares)| WithdrawalRequest {
                op_id,
                amount,
                receiver: String::from("receiver.near"),
                owner: String::from("owner.near"),
                escrow_shares,
            })
    }

    proptest! {
        // ===================================================================
        // Property: start_allocation from Idle succeeds
        // Invariant: Can start allocation from Idle with non-empty plan
        // ===================================================================
        #[test]
        fn start_allocation_from_idle_succeeds(
            plan in arb_plan(10),
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_allocation(OpState::Idle, plan.clone(), op_id);
            prop_assert!(result.is_ok());

            let result = result.unwrap();
            prop_assert!(result.new_state.is_allocating());

            let alloc = result.new_state.as_allocating().unwrap();
            prop_assert_eq!(alloc.op_id, op_id);
            prop_assert_eq!(alloc.index, 0);

            let expected_remaining: u128 = plan.iter().map(|(_, amt)| amt).sum();
            prop_assert_eq!(alloc.remaining, expected_remaining);
        }

        // ===================================================================
        // Property: Cannot double-start allocation
        // Invariant: start_allocation fails when already allocating
        // ===================================================================
        #[test]
        fn cannot_double_start_allocation(
            plan1 in arb_plan(5),
            plan2 in arb_plan(5),
            op_id1 in 1u64..=u64::MAX / 2,
            op_id2 in u64::MAX / 2 + 1..=u64::MAX,
        ) {
            // First start succeeds
            let result1 = start_allocation(OpState::Idle, plan1, op_id1).unwrap();
            prop_assert!(result1.new_state.is_allocating());

            // Second start from Allocating fails
            let result2 = start_allocation(result1.new_state, plan2, op_id2);
            prop_assert!(result2.is_err());
            let is_not_idle = matches!(result2, Err(TransitionError::NotIdle { .. }));
            prop_assert!(is_not_idle, "expected NotIdle error");
        }

        // ===================================================================
        // Property: start_withdrawal from Idle succeeds
        // Invariant: Can start withdrawal from Idle with valid request
        // ===================================================================
        #[test]
        fn start_withdrawal_from_idle_succeeds(
            request in arb_withdrawal_request(),
        ) {
            let result = start_withdrawal(OpState::Idle, request.clone());
            prop_assert!(result.is_ok());

            let result = result.unwrap();
            prop_assert!(result.new_state.is_withdrawing());

            let withdraw = result.new_state.as_withdrawing().unwrap();
            prop_assert_eq!(withdraw.op_id, request.op_id);
            prop_assert_eq!(withdraw.remaining, request.amount);
            prop_assert_eq!(withdraw.collected, 0);
            prop_assert_eq!(withdraw.escrow_shares, request.escrow_shares);
        }

        // ===================================================================
        // Property: Cannot double-start withdrawal
        // Invariant: start_withdrawal fails when already withdrawing
        // ===================================================================
        #[test]
        fn cannot_double_start_withdrawal(
            request1 in arb_withdrawal_request(),
            request2 in arb_withdrawal_request(),
        ) {
            // First start succeeds
            let result1 = start_withdrawal(OpState::Idle, request1).unwrap();
            prop_assert!(result1.new_state.is_withdrawing());

            // Second start from Withdrawing fails
            let result2 = start_withdrawal(result1.new_state, request2);
            prop_assert!(result2.is_err());
            let is_not_idle = matches!(result2, Err(TransitionError::NotIdle { .. }));
            prop_assert!(is_not_idle, "expected NotIdle error");
        }

        // ===================================================================
        // Property: start_refresh from Idle succeeds
        // Invariant: Can start refresh from Idle with non-empty plan
        // ===================================================================
        #[test]
        fn start_refresh_from_idle_succeeds(
            targets in proptest::collection::vec(0u32..100u32, 1..10),
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_refresh(OpState::Idle, targets.clone(), op_id);
            prop_assert!(result.is_ok());

            let result = result.unwrap();
            prop_assert!(result.new_state.is_refreshing());

            let refresh = result.new_state.as_refreshing().unwrap();
            prop_assert_eq!(refresh.op_id, op_id);
            prop_assert_eq!(refresh.index, 0);
            prop_assert_eq!(refresh.plan.clone(), targets);
        }

        // ===================================================================
        // Property: Allocation step advances index
        // Invariant: Successful step increments index and decrements remaining
        // ===================================================================
        #[test]
        fn allocation_step_advances_correctly(
            plan in arb_plan(5),
            op_id in 1u64..=u64::MAX,
            allocated in 1u128..=1_000_000u128,
        ) {
            let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
            let alloc = result.new_state.as_allocating().unwrap();
            let initial_remaining = alloc.remaining;

            let step_result = allocation_step_callback(result.new_state, true, allocated, op_id);
            prop_assert!(step_result.is_ok());

            let step_result = step_result.unwrap();
            let new_alloc = step_result.new_state.as_allocating().unwrap();

            prop_assert_eq!(new_alloc.index, 1);
            prop_assert_eq!(new_alloc.remaining, initial_remaining.saturating_sub(allocated));
        }

        // ===================================================================
        // Property: Allocation failure returns to Idle
        // Invariant: Failed allocation step returns to Idle
        // ===================================================================
        #[test]
        fn allocation_failure_returns_to_idle(
            plan in arb_plan(5),
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
            let step_result = allocation_step_callback(result.new_state, false, 0, op_id);

            prop_assert!(step_result.is_ok());
            prop_assert!(step_result.unwrap().new_state.is_idle());
        }

        // ===================================================================
        // Property: Op ID mismatch is rejected
        // Invariant: Callback with wrong op_id fails
        // ===================================================================
        #[test]
        fn op_id_mismatch_rejected(
            plan in arb_plan(3),
            op_id in 1u64..=u64::MAX / 2,
            wrong_op_id in u64::MAX / 2 + 1..=u64::MAX,
        ) {
            let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
            let step_result = allocation_step_callback(result.new_state, true, 100, wrong_op_id);

            prop_assert!(step_result.is_err());
            let is_op_id_mismatch = matches!(step_result, Err(TransitionError::OpIdMismatch { .. }));
            prop_assert!(is_op_id_mismatch, "expected OpIdMismatch error");
        }

        // ===================================================================
        // Property: complete_allocation to Idle without pending
        // Invariant: Completes to Idle when no pending withdrawal
        // ===================================================================
        #[test]
        fn complete_allocation_to_idle(
            plan in arb_plan(3),
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
            let complete = complete_allocation(result.new_state, op_id, None);

            prop_assert!(complete.is_ok());
            prop_assert!(complete.unwrap().new_state.is_idle());
        }

        // ===================================================================
        // Property: complete_allocation to Withdrawing with pending
        // Invariant: Completes to Withdrawing when pending withdrawal exists
        // ===================================================================
        #[test]
        fn complete_allocation_to_withdrawing(
            plan in arb_plan(3),
            op_id in 1u64..=u64::MAX / 2,
            pending in arb_withdrawal_request(),
        ) {
            let result = start_allocation(OpState::Idle, plan, op_id).unwrap();
            let complete = complete_allocation(result.new_state, op_id, Some(pending.clone()));

            prop_assert!(complete.is_ok());
            let new_state = complete.unwrap().new_state;
            prop_assert!(new_state.is_withdrawing());

            let withdraw = new_state.as_withdrawing().unwrap();
            prop_assert_eq!(withdraw.op_id, pending.op_id);
        }

        // ===================================================================
        // Property: Withdrawal step accumulates collected
        // Invariant: collected increases by amount_collected
        // ===================================================================
        #[test]
        fn withdrawal_step_accumulates_collected(
            request in arb_withdrawal_request(),
            collected1 in 1u128..=1_000_000u128,
            collected2 in 1u128..=1_000_000u128,
        ) {
            let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();

            let step1 = withdrawal_step_callback(result.new_state, request.op_id, collected1).unwrap();
            let w1 = step1.new_state.as_withdrawing().unwrap();
            prop_assert_eq!(w1.collected, collected1);
            prop_assert_eq!(w1.index, 1);

            let step2 = withdrawal_step_callback(step1.new_state, request.op_id, collected2).unwrap();
            let w2 = step2.new_state.as_withdrawing().unwrap();
            prop_assert_eq!(w2.collected, collected1.saturating_add(collected2));
            prop_assert_eq!(w2.index, 2);
        }

        // ===================================================================
        // Property: withdrawal_collected validates burn <= escrow
        // Invariant: Cannot burn more shares than escrowed
        // ===================================================================
        #[test]
        fn withdrawal_collected_validates_burn(
            request in arb_withdrawal_request(),
            excess in 1u128..=1_000_000u128,
        ) {
            let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
            let burn_shares = request.escrow_shares.saturating_add(excess);

            let collected = withdrawal_collected(result.new_state, request.op_id, burn_shares);
            prop_assert!(collected.is_err());
            let is_burn_exceeds = matches!(collected, Err(TransitionError::BurnExceedsEscrow { .. }));
            prop_assert!(is_burn_exceeds, "expected BurnExceedsEscrow error");
        }

        // ===================================================================
        // Property: stop_withdrawal returns to Idle
        // Invariant: Stopping returns to Idle with refund effects
        // ===================================================================
        #[test]
        fn stop_withdrawal_returns_to_idle(
            request in arb_withdrawal_request(),
        ) {
            let result = start_withdrawal(OpState::Idle, request.clone()).unwrap();
            let stop = stop_withdrawal(result.new_state, request.op_id);

            prop_assert!(stop.is_ok());
            prop_assert!(stop.unwrap().new_state.is_idle());
        }

        // ===================================================================
        // Property: complete_refresh returns to Idle
        // Invariant: Refresh completion returns to Idle
        // ===================================================================
        #[test]
        fn complete_refresh_returns_to_idle(
            targets in proptest::collection::vec(0u32..100u32, 1..10),
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_refresh(OpState::Idle, targets, op_id).unwrap();
            let complete = complete_refresh(result.new_state, op_id);

            prop_assert!(complete.is_ok());
            prop_assert!(complete.unwrap().new_state.is_idle());
        }

        // ===================================================================
        // Property: payout_complete returns to Idle
        // Invariant: Both success and failure complete to Idle
        // ===================================================================
        #[test]
        fn payout_complete_returns_to_idle(
            op_id in 1u64..=u64::MAX,
            amount in 1u128..=1_000_000_000u128,
            escrow_shares in 1u128..=1_000_000_000u128,
            burn_pct in 0u8..=100u8,
            success in proptest::bool::ANY,
        ) {
            let burn_shares = (escrow_shares as u128 * burn_pct as u128) / 100;
            let payout = PayoutState {
                op_id,
                receiver: String::from("receiver"),
                amount,
                owner: String::from("owner"),
                escrow_shares,
                burn_shares,
            };
            let state = OpState::Payout(payout);

            let result = payout_complete(state, success, op_id);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap().new_state.is_idle());
        }

        // ===================================================================
        // Property: Zero withdrawal amount is rejected
        // Invariant: Cannot start withdrawal with zero amount
        // ===================================================================
        #[test]
        fn zero_withdrawal_amount_rejected(
            op_id in 1u64..=u64::MAX,
            escrow_shares in 1u128..=1_000_000u128,
        ) {
            let request = WithdrawalRequest {
                op_id,
                amount: 0,
                receiver: String::from("receiver"),
                owner: String::from("owner"),
                escrow_shares,
            };
            let result = start_withdrawal(OpState::Idle, request);

            prop_assert!(result.is_err());
            prop_assert!(matches!(result, Err(TransitionError::ZeroWithdrawalAmount)));
        }

        // ===================================================================
        // Property: Zero escrow shares is rejected
        // Invariant: Cannot start withdrawal with zero escrow shares
        // ===================================================================
        #[test]
        fn zero_escrow_shares_rejected(
            op_id in 1u64..=u64::MAX,
            amount in 1u128..=1_000_000u128,
        ) {
            let request = WithdrawalRequest {
                op_id,
                amount,
                receiver: String::from("receiver"),
                owner: String::from("owner"),
                escrow_shares: 0,
            };
            let result = start_withdrawal(OpState::Idle, request);

            prop_assert!(result.is_err());
            prop_assert!(matches!(result, Err(TransitionError::ZeroEscrowShares)));
        }

        // ===================================================================
        // Property: Empty allocation plan is rejected
        // Invariant: Cannot start allocation with empty plan
        // ===================================================================
        #[test]
        fn empty_allocation_plan_rejected(
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_allocation(OpState::Idle, vec![], op_id);

            prop_assert!(result.is_err());
            prop_assert!(matches!(result, Err(TransitionError::EmptyAllocationPlan)));
        }

        // ===================================================================
        // Property: Empty refresh plan is rejected
        // Invariant: Cannot start refresh with empty plan
        // ===================================================================
        #[test]
        fn empty_refresh_plan_rejected(
            op_id in 1u64..=u64::MAX,
        ) {
            let result = start_refresh(OpState::Idle, vec![], op_id);

            prop_assert!(result.is_err());
            prop_assert!(matches!(result, Err(TransitionError::EmptyRefreshPlan)));
        }
    }
}
