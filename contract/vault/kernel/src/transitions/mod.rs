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
//! Withdrawing -> Payout (withdrawal_collected / withdrawal_settled, only
//!                     when the queue head's own epoch has an accepted
//!                     settlement snapshot, at the derived claim and with the
//!                     request's full escrow burned)
//! Withdrawing -> Idle (stop_withdrawal)
//! Refreshing -> Idle (complete_refresh)
//! Payout -> Idle (payout_complete, only when the stored payout still matches
//!                the head's derived settled claim and exact full escrow burn)
//! ```

use alloc::vec;
use alloc::vec::Vec;

use crate::effects::{KernelEffect, KernelEvent};
use crate::state::op_state::{
    AllocatingState, AllocationPlanEntry, OpState, PayoutState, RefreshingState, TargetId,
    WithdrawingState,
};
use crate::types::Address;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::{settled_claim, MIN_WITHDRAWAL_ASSETS};
#[cfg(feature = "action-epoch-settlement")]
use crate::state::vault::VaultState;

/// Error types for state transitions.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum TransitionError {
    WrongState,
    OpIdMismatch { expected: u64, actual: u64 },
    EmptyAllocationPlan,
    EmptyRefreshPlan,
    ZeroWithdrawalAmount,
    ZeroEscrowShares,
    InvalidIndex { index: u32, max: u32 },
    CollectionOverflow { collected: u128, remaining: u128 },
    AllocationOverflow { allocated: u128, remaining: u128 },
    ZeroAllocationAmount,
    BurnExceedsEscrow { burn: u128, escrow: u128 },
    WithdrawalIncomplete { remaining: u128, collected: u128 },
}

impl TransitionError {
    /// Get the name of an OpState variant as a static string.
    #[allow(dead_code)]
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn state_name(state: &OpState) -> &'static str {
        state.kind_name()
    }
}

/// Validate that a plan step index is within bounds.
#[inline]
fn validate_plan_index(index: u32, plan_len: usize) -> Result<(), TransitionError> {
    let len = plan_len as u32;
    if index >= len {
        return Err(TransitionError::InvalidIndex {
            index,
            max: len.saturating_sub(1),
        });
    }
    Ok(())
}

/// Result of a successful state transition.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
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

/// Extract the inner state of a specific OpState variant, or return a typed error.
/// Takes ownership of the state to avoid unnecessary clones.
macro_rules! require_state {
    ($state:expr, $variant:ident) => {
        match $state {
            OpState::$variant(s) => s,
            _ => {
                return Err(TransitionError::WrongState);
            }
        }
    };
}

/// Assert the OpState is Idle, or return WrongState.
macro_rules! require_idle {
    ($state:expr) => {
        if !$state.is_idle() {
            return Err(TransitionError::WrongState);
        }
    };
}

// Allocation Transitions

/// Start an allocation from Idle state.
///
/// # Arguments
/// * `state` - Current state (must be Idle)
/// * `plan` - Allocation steps specifying where to allocate
/// * `op_id` - Unique operation ID for correlation
///
/// # Returns
/// * `Ok(TransitionResult)` with new Allocating state
/// * `Err(TransitionError::WrongState)` if not in Idle state
/// * `Err(TransitionError::EmptyAllocationPlan)` if plan is empty
pub fn start_allocation(
    state: OpState,
    plan: Vec<AllocationPlanEntry>,
    op_id: u64,
) -> TransitionRes {
    require_idle!(state);

    if plan.is_empty() {
        return Err(TransitionError::EmptyAllocationPlan);
    }

    let mut total = 0u128;
    for step in &plan {
        total = total.saturating_add(step.amount);
    }

    let plan_len = plan.len() as u32;
    let new_state = OpState::Allocating(AllocatingState {
        op_id,
        index: 0,
        remaining: total,
        plan,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::AllocationStarted {
                op_id,
                total,
                plan_len,
            },
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
    let alloc = match state {
        OpState::Allocating(alloc) => alloc,
        other => {
            let _ = other;
            return Err(TransitionError::WrongState);
        }
    };

    if alloc.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: alloc.op_id,
            actual: op_id,
        });
    }

    validate_plan_index(alloc.index, alloc.plan.len())?;

    if !success {
        // On failure, return to Idle.
        // Compute total_allocated so caller can restore idle_assets correctly.
        let mut original_total = 0u128;
        for step in &alloc.plan {
            original_total = original_total.saturating_add(step.amount);
        }
        let total_allocated = original_total.saturating_sub(alloc.remaining);

        return Ok(TransitionResult::with_effects(
            OpState::Idle,
            vec![KernelEffect::EmitEvent {
                event: KernelEvent::AllocationStepFailed {
                    op_id: alloc.op_id,
                    index: alloc.index,
                    remaining: alloc.remaining,
                    total_allocated,
                },
            }],
        ));
    }

    // Reject zero allocation on success - prevents malicious markets from
    // advancing allocation steps without actually allocating.
    if amount_allocated == 0 {
        return Err(TransitionError::ZeroAllocationAmount);
    }

    if amount_allocated > alloc.remaining {
        return Err(TransitionError::AllocationOverflow {
            allocated: amount_allocated,
            remaining: alloc.remaining,
        });
    }

    Ok(TransitionResult::new(OpState::Allocating(
        alloc.advance(amount_allocated),
    )))
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
    let alloc = require_state!(state, Allocating);

    if alloc.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: alloc.op_id,
            actual: op_id,
        });
    }

    // Only chain into withdrawal when the pending request is actionable.
    // Zero-amount requests are handled by the caller's queue-skip path once Idle.
    let actionable_withdrawal = pending_withdrawal.filter(|req| req.amount > 0);

    match actionable_withdrawal {
        Some(req) => {
            if req.escrow_shares == 0 {
                return Err(TransitionError::ZeroEscrowShares);
            }

            // Transition to Withdrawing to process the pending request
            let new_state = OpState::Withdrawing(WithdrawingState {
                op_id: req.op_id,
                request_id: req.request_id,
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
                    event: KernelEvent::AllocationCompleted {
                        op_id,
                        has_withdrawal: true,
                    },
                }],
            ))
        }
        None => {
            // No pending withdrawal, return to Idle
            Ok(TransitionResult::with_effects(
                OpState::Idle,
                vec![KernelEffect::EmitEvent {
                    event: KernelEvent::AllocationCompleted {
                        op_id,
                        has_withdrawal: false,
                    },
                }],
            ))
        }
    }
}

// Withdrawal Transitions

/// Request for a withdrawal operation.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct WithdrawalRequest {
    /// Unique operation ID.
    pub op_id: u64,
    /// Unique queue request ID.
    pub request_id: u64,
    /// Amount of assets to withdraw.
    pub amount: u128,
    /// Receiver of the assets.
    pub receiver: Address,
    /// Owner of the shares being redeemed.
    pub owner: Address,
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
    require_idle!(state);

    if request.amount == 0 {
        return Err(TransitionError::ZeroWithdrawalAmount);
    }

    if request.escrow_shares == 0 {
        return Err(TransitionError::ZeroEscrowShares);
    }

    let new_state = OpState::Withdrawing(WithdrawingState {
        op_id: request.op_id,
        request_id: request.request_id,
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
            event: KernelEvent::WithdrawalStarted {
                op_id: request.op_id,
                amount: request.amount,
                escrow_shares: request.escrow_shares,
                owner: request.owner,
                receiver: request.receiver,
            },
        }],
    ))
}

/// Advance withdrawal by recording collected funds.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `op_id` - Operation ID to verify correlation
/// * `escrow_address` - Address holding escrowed shares
/// * `amount_collected` - Amount collected in this step
///
/// # Returns
/// * `Ok(TransitionResult)` with updated Withdrawing state or Payout state
pub fn withdrawal_step_callback(
    state: OpState,
    op_id: u64,
    amount_collected: u128,
) -> TransitionRes {
    let withdraw = require_state!(state, Withdrawing);

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    if amount_collected > withdraw.remaining {
        return Err(TransitionError::CollectionOverflow {
            collected: amount_collected,
            remaining: withdraw.remaining,
        });
    }

    Ok(TransitionResult::new(OpState::Withdrawing(
        withdraw.advance(amount_collected),
    )))
}

#[cfg(feature = "action-epoch-settlement")]
/// Details of a payout whose settlement law was verified against accepted
/// epoch state. Only [`Self::enforce`] can produce one.
pub struct PayoutEscrowDetails {
    amount: u128,
    burn_shares: u128,
    refund_shares: u128,
}

#[cfg(feature = "action-epoch-settlement")]
impl PayoutEscrowDetails {
    /// Recompute the settlement law for the stored payout and return the
    /// only permitted settlement details.
    ///
    /// The payout must still belong to the current FIFO queue head, the
    /// head's own epoch must have an immutable accepted settlement
    /// snapshot, the payout amount must equal the claim derived from that
    /// snapshot, the claim must satisfy the request's minimum and the
    /// protocol floor, and success must burn the request's full escrow.
    /// Every violation is rejected before state or accounting changes.
    fn enforce(
        payout: &PayoutState,
        vault: &VaultState,
        min_withdrawal_assets: u128,
    ) -> Result<Self, TransitionError> {
        let (head_id, head) = vault
            .withdraw_queue
            .head()
            .ok_or(TransitionError::WithdrawalIncomplete {
                remaining: 0,
                collected: payout.amount,
            })?;
        if head_id != payout.request_id
            || head.owner != payout.owner
            || head.receiver != payout.receiver
            || head.escrow_shares != payout.escrow_shares
        {
            return Err(TransitionError::WrongState);
        }

        let claim =
            settled_claim(head, &vault.epoch).ok_or(TransitionError::WithdrawalIncomplete {
                remaining: 0,
                collected: payout.amount,
            })?;
        let floor = head
            .min_assets_out
            .max(min_withdrawal_assets)
            .max(MIN_WITHDRAWAL_ASSETS);
        if claim == 0 || claim < floor {
            return Err(TransitionError::WithdrawalIncomplete {
                remaining: floor.saturating_sub(claim),
                collected: claim,
            });
        }

        if payout.amount != claim {
            return Err(TransitionError::WithdrawalIncomplete {
                remaining: payout.amount.saturating_sub(claim),
                collected: claim,
            });
        }
        if payout.burn_shares != payout.escrow_shares {
            return Err(TransitionError::BurnExceedsEscrow {
                burn: payout.burn_shares,
                escrow: payout.escrow_shares,
            });
        }
        Ok(Self {
            amount: claim,
            burn_shares: payout.escrow_shares,
            refund_shares: payout.escrow_shares,
        })
    }
}

#[cfg(feature = "action-epoch-settlement")]
/// Authorize the only lawful transition from `Withdrawing` to `Payout`.
///
/// The payout amount is not a caller input. It is derived exclusively from
/// the queue head's own accepted epoch settlement, and the request's full
/// escrow is the only permitted burn. Every check completes before any
/// payout state or effect is constructed, so a rejected attempt leaves the
/// unsettled head queued and vault accounting untouched.
fn authorize_settled_payout(
    state: &OpState,
    vault: &VaultState,
    op_id: u64,
    min_withdrawal_assets: u128,
) -> Result<PayoutState, TransitionError> {
    let withdraw = match state {
        OpState::Withdrawing(s) => s,
        _ => return Err(TransitionError::WrongState),
    };

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    if withdraw.remaining > 0 {
        return Err(TransitionError::WithdrawalIncomplete {
            remaining: withdraw.remaining,
            collected: withdraw.collected,
        });
    }

    // The payout may only serve the current FIFO head, and the withdrawing
    // operation must be the queued request itself.
    let (head_id, head) = vault
        .withdraw_queue
        .head()
        .ok_or(TransitionError::WithdrawalIncomplete {
            remaining: 0,
            collected: 0,
        })?;
    if head_id != withdraw.request_id
        || head.owner != withdraw.owner
        || head.receiver != withdraw.receiver
        || head.escrow_shares != withdraw.escrow_shares
    {
        return Err(TransitionError::WrongState);
    }

    // The head's own epoch must have an immutable accepted settlement
    // snapshot; the claim is recomputed from that snapshot, never stored or
    // accepted from a caller.
    let claim = settled_claim(head, &vault.epoch).ok_or(TransitionError::WithdrawalIncomplete {
        remaining: 0,
        collected: withdraw.collected,
    })?;
    let floor = head
        .min_assets_out
        .max(min_withdrawal_assets)
        .max(MIN_WITHDRAWAL_ASSETS);
    if claim == 0 || claim < floor {
        return Err(TransitionError::WithdrawalIncomplete {
            remaining: floor.saturating_sub(claim),
            collected: claim,
        });
    }
    if withdraw.collected != claim {
        return Err(TransitionError::WithdrawalIncomplete {
            remaining: withdraw.collected.saturating_sub(claim),
            collected: claim,
        });
    }

    // Payable only in full from idle assets; partial liquidity must not
    // construct a partial payout.
    if claim > vault.idle_assets {
        return Err(TransitionError::WithdrawalIncomplete {
            remaining: claim - vault.idle_assets,
            collected: withdraw.collected,
        });
    }

    Ok(PayoutState {
        op_id: withdraw.op_id,
        request_id: withdraw.request_id,
        receiver: withdraw.receiver,
        amount: claim,
        owner: withdraw.owner,
        escrow_shares: head.escrow_shares,
        burn_shares: head.escrow_shares,
    })
}

#[cfg(feature = "action-epoch-settlement")]
fn settlement_effects(payout: &PayoutState) -> TransitionResult {
    TransitionResult::with_effects(
        OpState::Payout(payout.clone()),
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalCollected {
                op_id: payout.op_id,
                burn_shares: payout.burn_shares,
                collected: payout.amount,
            },
        }],
    )
}

#[cfg(feature = "action-epoch-settlement")]
/// Authorized transition `Withdrawing -> Payout` under the fixed settlement
/// law.
///
/// The queue head's own epoch must have an immutable accepted settlement
/// snapshot. The payout amount is derived only from that snapshot
/// (`floor(head.escrow_shares * settlement_nav / eligible_supply)`, subject
/// to the request's stored minimum and the protocol floor), and the request
/// is authorized only when its full escrow is burned exactly once. Amount
/// and burn values are no longer caller inputs; a wrong, below-floor, or
/// unsettled claim is rejected before any payout or accounting change, and
/// the unsettled head remains queued.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `vault` - Full vault state providing the withdrawal queue and epoch
///   settlement context used to derive the settled claim
/// * `op_id` - Operation ID to verify correlation
/// * `min_withdrawal_assets` - Protocol payout floor applied to the derived
///   claim
///
/// # Returns
/// * `Ok(TransitionResult)` with Payout state only at the derived claim and
///   full escrow burn
/// * `Err` before constructing Payout on any settlement-law violation
pub fn withdrawal_collected(
    state: OpState,
    vault: &VaultState,
    op_id: u64,
    min_withdrawal_assets: u128,
) -> TransitionRes {
    let payout = authorize_settled_payout(&state, vault, op_id, min_withdrawal_assets)?;
    Ok(settlement_effects(&payout))
}

#[cfg(not(feature = "action-epoch-settlement"))]
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
    let withdraw = require_state!(state, Withdrawing);

    if withdraw.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: withdraw.op_id,
            actual: op_id,
        });
    }

    if withdraw.remaining > 0 {
        return Err(TransitionError::WithdrawalIncomplete {
            remaining: withdraw.remaining,
            collected: withdraw.collected,
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
        request_id: withdraw.request_id,
        receiver: withdraw.receiver,
        amount: withdraw.collected,
        owner: withdraw.owner,
        escrow_shares: withdraw.escrow_shares,
        burn_shares,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalCollected {
                op_id,
                burn_shares,
                collected: withdraw.collected,
            },
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
pub fn stop_withdrawal(state: OpState, op_id: u64, escrow_address: Address) -> TransitionRes {
    let withdraw = require_state!(state, Withdrawing);

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
        let owner_address = withdraw.owner;
        effects.push(KernelEffect::TransferShares {
            from: escrow_address,
            to: owner_address,
            shares: withdraw.escrow_shares,
        });
    }

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::WithdrawalStopped {
            op_id,
            escrow_shares: withdraw.escrow_shares,
        },
    });

    Ok(TransitionResult::with_effects(OpState::Idle, effects))
}

#[cfg(feature = "action-epoch-settlement")]
/// Transition `Withdrawing -> Payout` under the fixed settlement law.
///
/// The payout amount is neither stored nor accepted from the caller: it is
/// exactly `floor(head.escrow_shares * settlement_nav / eligible_supply)`
/// recomputed from the immutable accepted snapshot of the queue head's own
/// epoch, subject to the request's stored minimum and the protocol floor,
/// and the request's full escrow is the only permitted burn. An unsettled,
/// wrong-epoch, below-floor, or mismatched head stays queued and unpriced,
/// with no state or accounting change.
///
/// Collection progress is recorded only through `withdrawal_step_callback`.
/// This transition authorizes payout for a request whose collection is
/// already complete at exactly the derived settled claim.
///
/// # Arguments
/// * `state` - Current state (must be Withdrawing)
/// * `vault` - Full vault state providing the withdrawal queue and epoch
///   settlement context used to derive the settled claim
/// * `op_id` - Operation ID to verify correlation
/// * `min_withdrawal_assets` - Protocol payout floor applied to the derived
///   claim
///
/// # Returns
/// * `Ok(TransitionResult)` with Payout state only at the derived claim and
///   full escrow burn
/// * `Err` before constructing Payout on any settlement-law violation
pub fn withdrawal_settled(
    state: OpState,
    vault: &VaultState,
    op_id: u64,
    min_withdrawal_assets: u128,
) -> TransitionRes {
    let payout = authorize_settled_payout(&state, vault, op_id, min_withdrawal_assets)?;
    Ok(settlement_effects(&payout))
}

#[cfg(not(feature = "action-epoch-settlement"))]
pub fn withdrawal_settled(
    state: OpState,
    op_id: u64,
    amount_collected: u128,
    burn_shares: u128,
) -> TransitionRes {
    let stepped = withdrawal_step_callback(state, op_id, amount_collected)?;
    let withdraw = require_state!(stepped.new_state, Withdrawing);

    if burn_shares > withdraw.escrow_shares {
        return Err(TransitionError::BurnExceedsEscrow {
            burn: burn_shares,
            escrow: withdraw.escrow_shares,
        });
    }

    let new_state = OpState::Payout(PayoutState {
        op_id: withdraw.op_id,
        request_id: withdraw.request_id,
        receiver: withdraw.receiver,
        amount: withdraw.collected,
        owner: withdraw.owner,
        escrow_shares: withdraw.escrow_shares,
        burn_shares,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::WithdrawalCollected {
                op_id,
                burn_shares,
                collected: withdraw.collected,
            },
        }],
    ))
}

// Refresh Transitions

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
    require_idle!(state);

    if plan.is_empty() {
        return Err(TransitionError::EmptyRefreshPlan);
    }

    let plan_len = plan.len() as u32;
    let new_state = OpState::Refreshing(RefreshingState {
        op_id,
        index: 0,
        plan,
    });

    Ok(TransitionResult::with_effects(
        new_state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::RefreshStarted { op_id, plan_len },
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
    let refresh = match state {
        OpState::Refreshing(refresh) => refresh,
        other => {
            let _ = other;
            return Err(TransitionError::WrongState);
        }
    };

    if refresh.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: refresh.op_id,
            actual: op_id,
        });
    }

    validate_plan_index(refresh.index, refresh.plan.len())?;

    Ok(TransitionResult::new(OpState::Refreshing(
        refresh.advance(),
    )))
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
    let refresh = require_state!(state, Refreshing);

    if refresh.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: refresh.op_id,
            actual: op_id,
        });
    }

    Ok(TransitionResult::with_effects(
        OpState::Idle,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::RefreshCompleted { op_id },
        }],
    ))
}

// Payout Transitions

#[cfg(feature = "action-epoch-settlement")]
/// Complete payout and return to Idle after enforcing the settlement law.
///
/// Enforcement is recomputed at settlement time: the stored payout must
/// still belong to the current FIFO queue head, the head's own epoch must
/// still have an immutable accepted settlement snapshot, the payout amount
/// must equal the claim derived from that snapshot, the claim must satisfy
/// the request minimum and the protocol floor, and success must burn the
/// request's full escrow exactly once. A rejected attempt emits no burn,
/// no transfer, and no idle-asset change, and the unsettled head remains
/// queued with accounting unchanged.
///
/// # Arguments
/// * `state` - Current state (must be Payout)
/// * `vault` - Full vault state providing the withdrawal queue and epoch
///   settlement context used to re-derive the settled claim
/// * `success` - Whether the transfer succeeded
/// * `op_id` - Operation ID to verify correlation
/// * `escrow_address` - Address holding escrowed shares
/// * `min_withdrawal_assets` - Protocol payout floor applied to the derived
///   claim
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle state and law-bound effects
pub fn payout_complete(
    state: OpState,
    vault: &VaultState,
    success: bool,
    op_id: u64,
    escrow_address: Address,
    min_withdrawal_assets: u128,
) -> TransitionRes {
    let payout = require_state!(state, Payout);

    if payout.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: payout.op_id,
            actual: op_id,
        });
    }

    let law = PayoutEscrowDetails::enforce(&payout, vault, min_withdrawal_assets)?;

    let mut effects = vec![];

    if success {
        // A lawful payout burns exactly the request's full escrow, once,
        // and refunds nothing.
        if law.burn_shares > 0 {
            effects.push(KernelEffect::BurnShares {
                owner: escrow_address,
                shares: law.burn_shares,
            });
        }
    } else {
        // A failed transfer refunds all escrowed shares and burns none.
        if law.refund_shares > 0 {
            effects.push(KernelEffect::TransferShares {
                from: escrow_address,
                to: payout.owner,
                shares: law.refund_shares,
            });
        }
    }

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::PayoutCompleted {
            op_id,
            success,
            burn_shares: if success { law.burn_shares } else { 0 },
            refund_shares: if success { 0 } else { law.refund_shares },
            amount: if success { law.amount } else { 0 },
        },
    });

    Ok(TransitionResult::with_effects(OpState::Idle, effects))
}

#[cfg(not(feature = "action-epoch-settlement"))]
/// Complete payout and return to Idle.
///
/// # Arguments
/// * `state` - Current state (must be Payout)
/// * `success` - Whether the transfer succeeded
/// * `op_id` - Operation ID to verify correlation
/// * `escrow_address` - Address holding escrowed shares
///
/// # Returns
/// * `Ok(TransitionResult)` with Idle state and appropriate effects
pub fn payout_complete(
    state: OpState,
    success: bool,
    op_id: u64,
    escrow_address: Address,
) -> TransitionRes {
    let payout = require_state!(state, Payout);

    if payout.op_id != op_id {
        return Err(TransitionError::OpIdMismatch {
            expected: payout.op_id,
            actual: op_id,
        });
    }

    let mut effects = vec![];

    let owner_address = payout.owner;

    let mut burn_shares = 0u128;
    let mut refund_shares = 0u128;
    let mut amount = 0u128;

    if success {
        // Defense-in-depth: validate burn <= escrow (should be enforced at Payout creation)
        if payout.burn_shares > payout.escrow_shares {
            return Err(TransitionError::BurnExceedsEscrow {
                burn: payout.burn_shares,
                escrow: payout.escrow_shares,
            });
        }

        // Burn the designated shares
        if payout.burn_shares > 0 {
            burn_shares = payout.burn_shares;
            effects.push(KernelEffect::BurnShares {
                owner: escrow_address,
                shares: payout.burn_shares,
            });
        }

        // Refund any remaining escrow shares (subtraction is safe after validation)
        refund_shares = payout.escrow_shares - payout.burn_shares;
        if refund_shares > 0 {
            effects.push(KernelEffect::TransferShares {
                from: escrow_address,
                to: owner_address,
                shares: refund_shares,
            });
        }

        amount = payout.amount;
    } else {
        // On failure, refund all escrow shares
        if payout.escrow_shares > 0 {
            refund_shares = payout.escrow_shares;
            effects.push(KernelEffect::TransferShares {
                from: escrow_address,
                to: owner_address,
                shares: payout.escrow_shares,
            });
        }
    }

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::PayoutCompleted {
            op_id,
            success,
            burn_shares,
            refund_shares,
            amount,
        },
    });

    Ok(TransitionResult::with_effects(OpState::Idle, effects))
}

#[cfg(test)]
mod tests;
