//! Kernel action dispatch for vault state transitions.
//!
//! This module defines the public `KernelAction` enum and a dispatcher that
//! applies actions to `VaultState` and returns effects.

extern crate alloc;

use core::mem;

use crate::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
use crate::error::{InvalidConfigCode, InvalidStateCode, KernelError};
use crate::{
    math::{
        number::Number,
        wad::{mul_div_ceil, mul_div_floor},
    },
    restrictions::{RestrictionKind, Restrictions},
};
use crate::{
    state::{
        op_state::{AllocationPlanEntry, OpState, PayoutState, TargetId},
        queue::{compute_idle_settlement, is_past_cooldown, QueueError, WithdrawQueue},
        vault::{VaultConfig, VaultState},
    },
    transitions::TransitionResult,
};
use crate::{
    transitions::{start_withdrawal, TransitionError, WithdrawalRequest},
    types::{Address, TimestampNs},
};
use alloc::vec;
use alloc::vec::Vec;

#[cfg(any(feature = "action-refresh-fees", test))]
use crate::math::wad::{
    compute_fee_shares_from_assets, compute_management_fee_shares, total_assets_for_fee_accrual,
};
#[cfg(any(feature = "action-refresh-fees", test))]
use crate::state::vault::FeeAccrualAnchor;

#[cfg(any(feature = "action-recovery", test))]
use crate::transitions::stop_withdrawal;

#[cfg(any(feature = "action-allocation-lifecycle", test))]
use crate::transitions::{complete_allocation, start_allocation};
#[cfg(any(feature = "action-refresh-lifecycle", test))]
use crate::transitions::{complete_refresh, start_refresh};

/// Result of applying a kernel action.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct KernelResult {
    pub state: VaultState,
    pub effects: Vec<KernelEffect>,
}

impl KernelResult {
    #[must_use]
    pub fn new(state: VaultState, effects: Vec<KernelEffect>) -> Self {
        Self { state, effects }
    }
}

/// Outcome for payout settlement.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub enum PayoutOutcome {
    Success,
    Failure,
}

/// Planned payout details for satisfying a queued withdrawal from idle assets.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct IdlePayoutPlan {
    pub op_id: u64,
    pub request_id: u64,
    pub receiver: Address,
    pub assets_out: u128,
    pub burn_shares: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
enum WithdrawalQueueOutcome {
    None,
    CoolingDown { requested_at_ns: TimestampNs },
    Ready(WithdrawalRequest),
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct PendingWithdrawalHead {
    id: u64,
    owner: Address,
    receiver: Address,
    escrow_shares: u128,
    expected_assets: u128,
    requested_at_ns: TimestampNs,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
enum WithdrawalHeadOutcome {
    Skip(WithdrawalSkipReason),
    CoolingDown { requested_at_ns: TimestampNs },
    Ready,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct PayoutSettlement {
    burn_shares: u128,
    refund_shares: u128,
    completed_amount: u128,
    success: bool,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct WithdrawalRequestPlan {
    owner: Address,
    receiver: Address,
    shares: u128,
    expected_assets: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ExternalAssetSyncPlan {
    new_external_assets: u128,
    new_total_assets: u128,
}

/// Plan an idle-funded payout from the current vault state.
///
/// Returns `Ok(None)` when the vault is in a valid withdrawing state but there is
/// not enough idle liquidity to satisfy the queue head yet.
pub fn plan_idle_payout(state: &VaultState) -> Result<Option<IdlePayoutPlan>, KernelError> {
    planning::plan_idle_payout(state)
}

/// Kernel actions supported by the dispatcher.
///
/// These actions drive the vault state machine. Each action validates preconditions,
/// updates state, and returns effects to be executed by the chain-specific runtime.
#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, PartialEq, Eq)]
pub enum KernelAction {
    /// Begin allocating idle assets to external markets according to a plan.
    ///
    /// Transition: Idle -> Allocating
    BeginAllocating {
        op_id: u64,
        plan: Vec<AllocationPlanEntry>,
        now_ns: TimestampNs,
    },

    /// Deposit assets into the vault and mint shares to the receiver.
    Deposit {
        owner: Address,
        receiver: Address,
        assets_in: u128,
        min_shares_out: u128,
        now_ns: TimestampNs,
    },

    AtomicWithdraw {
        owner: Address,
        receiver: Address,
        operator: Address,
        assets_out: u128,
        max_shares_burned: u128,
        now_ns: TimestampNs,
    },

    AtomicRedeem {
        owner: Address,
        receiver: Address,
        operator: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    },

    /// Request a withdrawal by escrowing shares in the queue.
    RequestWithdraw {
        owner: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    },

    /// Execute the next pending withdrawal from the queue.
    ///
    /// Transition: Idle -> Withdrawing
    ExecuteWithdraw { now_ns: TimestampNs },

    /// Begin refreshing external market balances.
    ///
    /// Transition: Idle -> Refreshing
    BeginRefreshing {
        op_id: u64,
        plan: Vec<TargetId>,
        now_ns: TimestampNs,
    },

    /// Complete an allocation operation.
    ///
    /// Transition: Allocating -> Idle or Withdrawing
    FinishAllocating { op_id: u64, now_ns: TimestampNs },

    /// Sync external asset balances during an active operation.
    SyncExternalAssets {
        new_external_assets: u128,
        op_id: u64,
        now_ns: TimestampNs,
    },

    RebalanceWithdraw {
        op_id: u64,
        amount: u128,
        now_ns: TimestampNs,
    },

    /// Complete a refresh operation.
    ///
    /// Transition: Refreshing -> Idle
    FinishRefreshing { op_id: u64, now_ns: TimestampNs },

    /// Abort a refresh operation (e.g., on external call failure).
    ///
    /// Transition: Refreshing -> Idle
    AbortRefreshing { op_id: u64 },

    /// Settle a payout after asset transfer attempt.
    ///
    /// Transition: Payout -> Idle
    SettlePayout { op_id: u64, outcome: PayoutOutcome },

    /// Abort an allocation operation (e.g., on external call failure).
    ///
    /// Transition: Allocating -> Idle
    AbortAllocating { op_id: u64 },

    /// Abort a withdrawal operation (e.g., on external call failure).
    ///
    /// Transition: Withdrawing -> Idle
    AbortWithdrawing { op_id: u64 },

    /// Refresh fee calculations and mint fee shares.
    RefreshFees { now_ns: TimestampNs },

    /// Emit a pause-state update for executor-owned pause configuration.
    Pause { paused: bool },

    /// Emergency reset: force the vault back to Idle from any non-Idle state.
    ///
    /// Unlike the regular abort actions, this does not require op_id matching.
    /// For Withdrawing/Payout states, escrowed shares are refunded to the owner
    /// and the queue head is dequeued.
    ///
    /// Authorization (Owner-only, timelock-gated) must be enforced by the executor.
    EmergencyReset,
}

impl KernelAction {
    #[must_use]
    pub fn begin_allocating(
        op_id: u64,
        plan: Vec<AllocationPlanEntry>,
        now_ns: TimestampNs,
    ) -> Self {
        Self::BeginAllocating {
            op_id,
            plan,
            now_ns,
        }
    }

    #[must_use]
    pub fn deposit(
        owner: Address,
        receiver: Address,
        assets_in: u128,
        min_shares_out: u128,
        now_ns: TimestampNs,
    ) -> Self {
        Self::Deposit {
            owner,
            receiver,
            assets_in,
            min_shares_out,
            now_ns,
        }
    }

    #[must_use]
    pub fn atomic_withdraw(
        owner: Address,
        receiver: Address,
        operator: Address,
        assets_out: u128,
        max_shares_burned: u128,
        now_ns: TimestampNs,
    ) -> Self {
        Self::AtomicWithdraw {
            owner,
            receiver,
            operator,
            assets_out,
            max_shares_burned,
            now_ns,
        }
    }

    #[must_use]
    pub fn atomic_redeem(
        owner: Address,
        receiver: Address,
        operator: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    ) -> Self {
        Self::AtomicRedeem {
            owner,
            receiver,
            operator,
            shares,
            min_assets_out,
            now_ns,
        }
    }

    #[must_use]
    pub fn request_withdraw(
        owner: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    ) -> Self {
        Self::RequestWithdraw {
            owner,
            receiver,
            shares,
            min_assets_out,
            now_ns,
        }
    }

    #[must_use]
    pub fn execute_withdraw(now_ns: TimestampNs) -> Self {
        Self::ExecuteWithdraw { now_ns }
    }

    #[must_use]
    pub fn begin_refreshing(op_id: u64, plan: Vec<TargetId>, now_ns: TimestampNs) -> Self {
        Self::BeginRefreshing {
            op_id,
            plan,
            now_ns,
        }
    }

    #[must_use]
    pub fn finish_allocating(op_id: u64, now_ns: TimestampNs) -> Self {
        Self::FinishAllocating { op_id, now_ns }
    }

    #[must_use]
    pub fn sync_external_assets(
        new_external_assets: u128,
        op_id: u64,
        now_ns: TimestampNs,
    ) -> Self {
        Self::SyncExternalAssets {
            new_external_assets,
            op_id,
            now_ns,
        }
    }

    #[must_use]
    pub fn rebalance_withdraw(op_id: u64, amount: u128, now_ns: TimestampNs) -> Self {
        Self::RebalanceWithdraw {
            op_id,
            amount,
            now_ns,
        }
    }

    #[must_use]
    pub fn finish_refreshing(op_id: u64, now_ns: TimestampNs) -> Self {
        Self::FinishRefreshing { op_id, now_ns }
    }

    #[must_use]
    pub fn abort_refreshing(op_id: u64) -> Self {
        Self::AbortRefreshing { op_id }
    }

    #[must_use]
    pub fn settle_payout(op_id: u64, outcome: PayoutOutcome) -> Self {
        Self::SettlePayout { op_id, outcome }
    }

    #[must_use]
    pub fn abort_allocating(op_id: u64) -> Self {
        Self::AbortAllocating { op_id }
    }

    #[must_use]
    pub fn abort_withdrawing(op_id: u64) -> Self {
        Self::AbortWithdrawing { op_id }
    }

    #[must_use]
    pub fn refresh_fees(now_ns: TimestampNs) -> Self {
        Self::RefreshFees { now_ns }
    }

    #[must_use]
    pub fn pause(paused: bool) -> Self {
        Self::Pause { paused }
    }

    #[must_use]
    pub const fn emergency_reset() -> Self {
        Self::EmergencyReset
    }

    #[must_use]
    pub const fn op_id(&self) -> Option<u64> {
        match self {
            Self::BeginAllocating { op_id, .. }
            | Self::BeginRefreshing { op_id, .. }
            | Self::FinishAllocating { op_id, .. }
            | Self::SyncExternalAssets { op_id, .. }
            | Self::RebalanceWithdraw { op_id, .. }
            | Self::FinishRefreshing { op_id, .. }
            | Self::AbortRefreshing { op_id }
            | Self::SettlePayout { op_id, .. }
            | Self::AbortAllocating { op_id, .. }
            | Self::AbortWithdrawing { op_id, .. } => Some(*op_id),
            Self::Deposit { .. }
            | Self::AtomicWithdraw { .. }
            | Self::AtomicRedeem { .. }
            | Self::RequestWithdraw { .. }
            | Self::ExecuteWithdraw { .. }
            | Self::RefreshFees { .. }
            | Self::Pause { .. }
            | Self::EmergencyReset => None,
        }
    }

    #[must_use]
    pub const fn timestamp_ns(&self) -> Option<TimestampNs> {
        match self {
            Self::BeginAllocating { now_ns, .. }
            | Self::Deposit { now_ns, .. }
            | Self::AtomicWithdraw { now_ns, .. }
            | Self::AtomicRedeem { now_ns, .. }
            | Self::RequestWithdraw { now_ns, .. }
            | Self::ExecuteWithdraw { now_ns }
            | Self::BeginRefreshing { now_ns, .. }
            | Self::FinishAllocating { now_ns, .. }
            | Self::SyncExternalAssets { now_ns, .. }
            | Self::RebalanceWithdraw { now_ns, .. }
            | Self::FinishRefreshing { now_ns, .. }
            | Self::RefreshFees { now_ns } => Some(*now_ns),
            Self::AbortRefreshing { .. }
            | Self::SettlePayout { .. }
            | Self::AbortAllocating { .. }
            | Self::AbortWithdrawing { .. }
            | Self::Pause { .. }
            | Self::EmergencyReset => None,
        }
    }
}

/// Effective totals after applying virtual share/asset offsets.
///
/// Named fields prevent callers from confusing supply vs assets.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EffectiveTotals {
    pub supply: u128,
    pub assets: u128,
}

/// Compute effective totals including virtual shares/assets for conversion math.
pub fn effective_totals(state: &VaultState, config: &VaultConfig) -> EffectiveTotals {
    conversions::effective_totals(state, config)
}

/// Convert an asset amount to shares (floor rounding — fewer shares, favors vault).
pub fn convert_to_shares(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    conversions::convert_to_shares(state, config, assets)
}

/// Convert a share amount to assets (floor rounding — fewer assets, favors vault).
pub fn convert_to_assets(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    conversions::convert_to_assets(state, config, shares)
}

/// Convert an asset amount to shares (ceil rounding — more shares, favors user).
///
/// Used by ERC-4626 `preview_withdraw` to compute shares burned (rounds against user).
pub fn convert_to_shares_ceil(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    conversions::convert_to_shares_ceil(state, config, assets)
}

/// Convert a share amount to assets (ceil rounding — more assets, favors user).
///
/// Used by ERC-4626 `preview_mint` to compute assets needed (rounds against user).
pub fn convert_to_assets_ceil(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    conversions::convert_to_assets_ceil(state, config, shares)
}

/// Preview the shares minted for a deposit of `assets` using kernel conversions.
#[inline]
#[must_use]
pub fn preview_deposit_shares(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    convert_to_shares(state, config, assets)
}

/// Preview the assets redeemed for `shares` using kernel conversions.
#[inline]
#[must_use]
pub fn preview_withdraw_assets(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    convert_to_assets(state, config, shares)
}

#[cfg(any(feature = "action-recovery", feature = "action-sync-external", test))]
fn require_active_op_id(
    op_state: &OpState,
    provided: u64,
    error_code: InvalidStateCode,
) -> Result<(), KernelError> {
    let active = match op_state.op_id() {
        Some(active) => active,
        None => return Err(KernelError::from(error_code)),
    };
    if active != provided {
        return Err(KernelError::OpIdMismatch {
            expected: active,
            actual: provided,
        });
    }
    Ok(())
}

/// Validate that a destructured op_id matches the provided one.
#[inline]
fn check_op_id(expected: u64, actual: u64) -> Result<(), KernelError> {
    if expected != actual {
        return Err(KernelError::OpIdMismatch { expected, actual });
    }
    Ok(())
}

/// Validate that the withdrawal queue head matches the expected owner/receiver/escrow.
///
/// Used by both `AbortWithdrawing` and `SettlePayout` to ensure consistency
/// between the op-state and the queue.
fn validate_queue_head(
    queue: &WithdrawQueue,
    request_id: u64,
    owner: &Address,
    receiver: &Address,
    escrow_shares: u128,
) -> Result<(), KernelError> {
    let Some((head_id, pending)) = queue.head() else {
        return Err(KernelError::NoPendingWithdrawals);
    };
    if head_id != request_id
        || pending.owner != *owner
        || pending.receiver != *receiver
        || pending.escrow_shares != escrow_shares
    {
        return Err(KernelError::from(
            InvalidStateCode::WithdrawalQueueHeadMismatch,
        ));
    }
    Ok(())
}

/// Push a `TransferShares` effect to refund escrowed shares to an owner.
///
/// No-op if `shares` is zero.
#[inline]
fn push_refund_shares(
    effects: &mut Vec<KernelEffect>,
    escrow: Address,
    owner: Address,
    shares: u128,
) {
    if shares > 0 {
        effects.push(KernelEffect::TransferShares {
            from: escrow,
            to: owner,
            shares,
        });
    }
}

#[cfg(any(feature = "action-refresh-fees", test))]
#[inline]
fn mint_fee_shares(
    effects: &mut Vec<KernelEffect>,
    total_supply: &mut u128,
    shares: Number,
    recipient: Address,
) -> Result<(), KernelError> {
    if shares > Number::zero() {
        let minted: u128 = shares.into();
        *total_supply = total_supply
            .checked_add(minted)
            .ok_or_else(|| KernelError::from(InvalidStateCode::FeeMintOverflowTotalSupply))?;
        effects.push(KernelEffect::MintShares {
            owner: recipient,
            shares: minted,
        });
    }
    Ok(())
}

#[inline]
fn map_transition_result<T>(result: Result<T, TransitionError>) -> Result<T, KernelError> {
    result.map_err(KernelError::Transition)
}

#[inline]
fn apply_transition_result(
    mut state: VaultState,
    result: Result<TransitionResult, TransitionError>,
) -> Result<KernelResult, KernelError> {
    let result = map_transition_result(result)?;
    state.op_state = result.new_state;
    Ok(KernelResult::new(state, result.effects))
}

#[inline]
fn map_queue_error(err: QueueError) -> KernelError {
    match err {
        QueueError::QueueFull { current, max } => KernelError::QueueFull { current, max },
        QueueError::CacheOverflow => {
            KernelError::from(InvalidStateCode::WithdrawalQueueCacheOverflow)
        }
        QueueError::WithdrawalNotFound { .. } => {
            KernelError::from(InvalidStateCode::WithdrawalQueueMissingEntry)
        }
        QueueError::QueueEmpty => KernelError::from(InvalidStateCode::UnexpectedEmptyQueue),
        QueueError::InvariantViolation { .. } => {
            KernelError::from(InvalidStateCode::WithdrawalQueueInvariantViolation)
        }
    }
}

/// Process a deposit: validate restrictions, convert assets→shares, update totals.
#[allow(clippy::too_many_arguments)]
fn handle_deposit(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: Address,
    receiver: Address,
    assets_in: u128,
    min_shares_out: u128,
) -> Result<KernelResult, KernelError> {
    enforce_restrictions(config, restrictions, self_id, &owner)?;
    enforce_restrictions(config, restrictions, self_id, &receiver)?;
    if !state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::DepositRequiresIdle));
    }
    if assets_in == 0 {
        return Err(KernelError::ZeroAmount);
    }

    let shares_out = convert_to_shares(&state, config, assets_in);
    if shares_out < min_shares_out {
        return Err(KernelError::Slippage {
            min: min_shares_out,
            actual: shares_out,
        });
    }

    state.total_assets = state
        .total_assets
        .checked_add(assets_in)
        .ok_or_else(|| KernelError::from(InvalidStateCode::DepositOverflowTotalAssets))?;
    state.idle_assets = state
        .idle_assets
        .checked_add(assets_in)
        .ok_or_else(|| KernelError::from(InvalidStateCode::DepositOverflowIdleAssets))?;
    state.total_shares = state
        .total_shares
        .checked_add(shares_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::MintOverflowTotalShares))?;

    let effects = vec![
        KernelEffect::TransferAssetsFrom {
            from: owner,
            to: *self_id,
            amount: assets_in,
        },
        KernelEffect::MintShares {
            owner: receiver,
            shares: shares_out,
        },
        KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::DepositProcessed {
                owner,
                receiver,
                assets_in,
                shares_out,
            },
        },
    ];

    Ok(KernelResult::new(state, effects))
}

#[inline]
fn push_atomic_burn_shares(
    effects: &mut Vec<KernelEffect>,
    owner: Address,
    operator: Address,
    shares: u128,
) {
    if operator == owner {
        effects.push(KernelEffect::BurnShares { owner, shares });
    } else {
        effects.push(KernelEffect::BurnSharesFrom {
            spender: operator,
            owner,
            shares,
        });
    }
}

#[inline]
fn enforce_withdrawal_actors(
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: &Address,
    receiver: &Address,
) -> Result<(), KernelError> {
    enforce_restrictions(config, restrictions, self_id, owner)?;
    enforce_restrictions(config, restrictions, self_id, receiver)
}

#[inline]
fn require_idle_with_nonzero_amount(
    state: &VaultState,
    idle_error: InvalidStateCode,
    amount: u128,
) -> Result<(), KernelError> {
    if !state.is_idle() {
        return Err(KernelError::from(idle_error));
    }
    if amount == 0 {
        return Err(KernelError::ZeroAmount);
    }
    Ok(())
}

#[inline]
fn restricted_withdraw_actor(
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: &Address,
    receiver: &Address,
) -> Option<RestrictionKind> {
    restrictions
        .and_then(|r| r.is_restricted(owner))
        .or_else(|| restrictions.and_then(|r| r.is_restricted_allowing_self(receiver, self_id)))
}

#[inline]
fn pending_withdrawal_skip_reason(
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: &Address,
    receiver: &Address,
    expected_assets: u128,
) -> Option<WithdrawalSkipReason> {
    if restricted_withdraw_actor(restrictions, self_id, owner, receiver).is_some() {
        Some(WithdrawalSkipReason::Restricted)
    } else if expected_assets == 0 {
        Some(WithdrawalSkipReason::ZeroExpectedAssets)
    } else {
        None
    }
}

fn dequeue_skipped_withdrawal(
    state: &mut VaultState,
    self_id: &Address,
    skipped_effects: &mut Vec<KernelEffect>,
    reason: WithdrawalSkipReason,
) -> Result<(), KernelError> {
    let (pending_id, pending) = state
        .withdraw_queue
        .dequeue()
        .ok_or(KernelError::NoPendingWithdrawals)?;
    push_refund_shares(
        skipped_effects,
        *self_id,
        pending.owner,
        pending.escrow_shares,
    );
    skipped_effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::WithdrawalSkipped {
            id: pending_id,
            owner: pending.owner,
            receiver: pending.receiver,
            escrow_shares: pending.escrow_shares,
            expected_assets: pending.expected_assets,
            reason,
        },
    });
    Ok(())
}

#[inline]
fn pending_withdrawal_head(state: &VaultState) -> Option<PendingWithdrawalHead> {
    state
        .withdraw_queue
        .head()
        .map(|(id, pending)| PendingWithdrawalHead {
            id,
            owner: pending.owner,
            receiver: pending.receiver,
            escrow_shares: pending.escrow_shares,
            expected_assets: pending.expected_assets,
            requested_at_ns: pending.requested_at_ns,
        })
}

#[inline]
fn classify_withdrawal_head(
    head: PendingWithdrawalHead,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    now_ns: TimestampNs,
) -> WithdrawalHeadOutcome {
    if let Some(reason) = pending_withdrawal_skip_reason(
        restrictions,
        self_id,
        &head.owner,
        &head.receiver,
        head.expected_assets,
    ) {
        WithdrawalHeadOutcome::Skip(reason)
    } else if !is_past_cooldown(head.requested_at_ns, now_ns, config.withdrawal_cooldown_ns) {
        WithdrawalHeadOutcome::CoolingDown {
            requested_at_ns: head.requested_at_ns,
        }
    } else {
        WithdrawalHeadOutcome::Ready
    }
}

#[inline]
fn withdrawal_request_from_head(
    state: &mut VaultState,
    head: PendingWithdrawalHead,
) -> WithdrawalRequest {
    WithdrawalRequest {
        op_id: state.allocate_op_id(),
        request_id: head.id,
        amount: head.expected_assets,
        receiver: head.receiver,
        owner: head.owner,
        escrow_shares: head.escrow_shares,
    }
}

#[inline]
fn plan_withdrawal_request(
    state: &VaultState,
    config: &VaultConfig,
    owner: Address,
    receiver: Address,
    shares: u128,
    min_assets_out: u128,
) -> Result<WithdrawalRequestPlan, KernelError> {
    let expected_assets = convert_to_assets(state, config, shares);
    if expected_assets < min_assets_out {
        return Err(KernelError::Slippage {
            min: min_assets_out,
            actual: expected_assets,
        });
    }
    if expected_assets < config.min_withdrawal_assets {
        return Err(KernelError::MinWithdrawal {
            amount: expected_assets,
            min: config.min_withdrawal_assets,
        });
    }

    Ok(WithdrawalRequestPlan {
        owner,
        receiver,
        shares,
        expected_assets,
    })
}

#[inline]
fn sync_external_in_flight_assets(op_state: &OpState) -> u128 {
    match op_state {
        OpState::Allocating(state) => state.remaining,
        _ => 0,
    }
}

#[inline]
fn ensure_sync_external_state_allowed(op_state: &OpState) -> Result<(), KernelError> {
    match op_state {
        OpState::Allocating(_) | OpState::Withdrawing(_) | OpState::Refreshing(_) => Ok(()),
        _ => Err(KernelError::from(
            InvalidStateCode::SyncExternalRequiresAllowedStates,
        )),
    }
}

#[inline]
fn plan_external_asset_sync(
    state: &VaultState,
    new_external_assets: u128,
) -> Result<ExternalAssetSyncPlan, KernelError> {
    let new_total_assets = state
        .idle_assets
        .checked_add(new_external_assets)
        .ok_or_else(|| KernelError::from(InvalidStateCode::SyncExternalOverflowIdlePlusExternal))?;

    let reference_total = state
        .total_assets
        .saturating_add(sync_external_in_flight_assets(&state.op_state));
    if reference_total > 0 && new_total_assets > reference_total.saturating_mul(2) {
        return Err(KernelError::from(
            InvalidStateCode::SyncExternalWouldMoreThanDoubleTotalAssets,
        ));
    }

    Ok(ExternalAssetSyncPlan {
        new_external_assets,
        new_total_assets,
    })
}

fn next_withdrawal_queue_outcome(
    state: &mut VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    now_ns: TimestampNs,
    skipped_effects: &mut Vec<KernelEffect>,
) -> Result<WithdrawalQueueOutcome, KernelError> {
    loop {
        let Some(head) = pending_withdrawal_head(state) else {
            return Ok(WithdrawalQueueOutcome::None);
        };

        match classify_withdrawal_head(head, config, restrictions, self_id, now_ns) {
            WithdrawalHeadOutcome::Skip(reason) => {
                dequeue_skipped_withdrawal(state, self_id, skipped_effects, reason)?;
            }
            WithdrawalHeadOutcome::CoolingDown { requested_at_ns } => {
                return Ok(WithdrawalQueueOutcome::CoolingDown { requested_at_ns });
            }
            WithdrawalHeadOutcome::Ready => {
                return Ok(WithdrawalQueueOutcome::Ready(withdrawal_request_from_head(
                    state, head,
                )));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_atomic_withdraw(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: Address,
    receiver: Address,
    operator: Address,
    assets_out: u128,
    max_shares_burned: u128,
) -> Result<KernelResult, KernelError> {
    enforce_withdrawal_actors(config, restrictions, self_id, &owner, &receiver)?;
    require_idle_with_nonzero_amount(
        &state,
        InvalidStateCode::AtomicWithdrawRequiresIdle,
        assets_out,
    )?;

    let shares = convert_to_shares_ceil(&state, config, assets_out);
    if shares == 0 {
        return Err(KernelError::ZeroAmount);
    }
    if shares > max_shares_burned {
        return Err(KernelError::Slippage {
            min: max_shares_burned,
            actual: shares,
        });
    }

    if assets_out > state.idle_assets {
        return Err(KernelError::from(
            InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
        ));
    }
    state.total_shares = state
        .total_shares
        .checked_sub(shares)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawBurnExceedsTotalShares))?;
    state.idle_assets = state
        .idle_assets
        .checked_sub(assets_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawExceedsIdleAssets))?;
    state.total_assets = state
        .total_assets
        .checked_sub(assets_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawTotalAssetsUnderflow))?;

    let mut effects = Vec::new();
    push_atomic_burn_shares(&mut effects, owner, operator, shares);
    effects.push(KernelEffect::TransferAssets {
        to: receiver,
        amount: assets_out,
    });
    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::AtomicWithdrawProcessed {
            owner,
            receiver,
            shares_burned: shares,
            assets_out,
        },
    });
    Ok(KernelResult::new(state, effects))
}

#[allow(clippy::too_many_arguments)]
fn handle_atomic_redeem(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: Address,
    receiver: Address,
    operator: Address,
    shares: u128,
    min_assets_out: u128,
) -> Result<KernelResult, KernelError> {
    enforce_withdrawal_actors(config, restrictions, self_id, &owner, &receiver)?;
    require_idle_with_nonzero_amount(&state, InvalidStateCode::AtomicWithdrawRequiresIdle, shares)?;

    let assets_out = convert_to_assets(&state, config, shares);
    if assets_out == 0 {
        return Err(KernelError::ZeroAmount);
    }
    if assets_out < min_assets_out {
        return Err(KernelError::Slippage {
            min: min_assets_out,
            actual: assets_out,
        });
    }
    if assets_out > state.idle_assets {
        return Err(KernelError::from(
            InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
        ));
    }

    state.total_shares = state
        .total_shares
        .checked_sub(shares)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawBurnExceedsTotalShares))?;
    state.idle_assets = state
        .idle_assets
        .checked_sub(assets_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawExceedsIdleAssets))?;
    state.total_assets = state
        .total_assets
        .checked_sub(assets_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::AtomicWithdrawTotalAssetsUnderflow))?;

    let mut effects = Vec::new();
    push_atomic_burn_shares(&mut effects, owner, operator, shares);
    effects.push(KernelEffect::TransferAssets {
        to: receiver,
        amount: assets_out,
    });
    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::AtomicWithdrawProcessed {
            owner,
            receiver,
            shares_burned: shares,
            assets_out,
        },
    });
    Ok(KernelResult::new(state, effects))
}

/// Enqueue a withdrawal request: validate, compute expected assets, escrow shares.
#[allow(clippy::too_many_arguments)]
fn handle_request_withdraw(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: Address,
    receiver: Address,
    shares: u128,
    min_assets_out: u128,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !config.is_max_pending_valid() {
        return Err(KernelError::from(
            InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
        ));
    }

    enforce_withdrawal_actors(config, restrictions, self_id, &owner, &receiver)?;
    require_idle_with_nonzero_amount(
        &state,
        InvalidStateCode::RequestWithdrawRequiresIdle,
        shares,
    )?;

    let request_plan =
        plan_withdrawal_request(&state, config, owner, receiver, shares, min_assets_out)?;

    let id = state
        .withdraw_queue
        .enqueue(
            request_plan.owner,
            request_plan.receiver,
            request_plan.shares,
            request_plan.expected_assets,
            now_ns,
            config.max_pending_withdrawals,
        )
        .map_err(map_queue_error)?;

    let effects = vec![
        KernelEffect::TransferShares {
            from: request_plan.owner,
            to: *self_id,
            shares: request_plan.shares,
        },
        KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::WithdrawalRequested {
                id,
                owner: request_plan.owner,
                receiver: request_plan.receiver,
                shares: request_plan.shares,
                expected_assets: request_plan.expected_assets,
            },
        },
    ];

    Ok(KernelResult::new(state, effects))
}

/// Execute the next queued withdrawal after cooldown.
fn handle_execute_withdraw(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !state.op_state.is_idle() {
        let error_code = if state.op_state.is_withdrawing() {
            InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks
        } else {
            InvalidStateCode::ExecuteWithdrawRequiresIdle
        };
        return Err(KernelError::from(error_code));
    }

    if is_globally_paused(config, restrictions) {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }

    let mut skipped_effects = Vec::new();
    match next_withdrawal_queue_outcome(
        &mut state,
        config,
        restrictions,
        self_id,
        now_ns,
        &mut skipped_effects,
    )? {
        WithdrawalQueueOutcome::None => {
            if skipped_effects.is_empty() {
                Err(KernelError::NoPendingWithdrawals)
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            }
        }
        WithdrawalQueueOutcome::CoolingDown { requested_at_ns } => {
            if skipped_effects.is_empty() {
                Err(KernelError::Cooldown {
                    requested_at: requested_at_ns.into(),
                    now: now_ns.into(),
                    cooldown_ns: config.withdrawal_cooldown_ns,
                })
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            }
        }
        WithdrawalQueueOutcome::Ready(request) => {
            let transition = start_withdrawal(mem::take(&mut state.op_state), request);
            let mut result = apply_transition_result(state, transition)?;
            skipped_effects.append(&mut result.effects);
            result.effects = skipped_effects;
            Ok(result)
        }
    }
}

/// Start an allocation: transition to Allocating and decrement idle assets.
#[cfg(any(feature = "action-allocation-lifecycle", test))]
fn handle_begin_allocating(
    mut state: VaultState,
    op_id: u64,
    plan: Vec<AllocationPlanEntry>,
) -> Result<KernelResult, KernelError> {
    let result = map_transition_result(start_allocation(
        mem::take(&mut state.op_state),
        plan,
        op_id,
    ))?;

    // Compute allocation total from the plan and decrement idle_assets.
    let alloc_total = match result.new_state.as_allocating() {
        Some(allocating) => allocating.remaining,
        None => {
            return Err(KernelError::from(
                InvalidStateCode::StartAllocationMustReturnAllocating,
            ))
        }
    };

    if alloc_total > state.idle_assets {
        return Err(KernelError::from(
            InvalidStateCode::AllocationPlanExceedsIdleAssets,
        ));
    }

    state.idle_assets -= alloc_total;
    state.sync_total_assets();
    state.op_state = result.new_state;
    Ok(KernelResult::new(state, result.effects))
}

/// Finish an allocation, optionally chaining into a pending withdrawal.
#[cfg(any(feature = "action-allocation-lifecycle", test))]
fn handle_finish_allocating(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    op_id: u64,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    let mut skipped_effects = Vec::new();

    let pending_req = if is_globally_paused(config, restrictions) {
        None
    } else {
        match next_withdrawal_queue_outcome(
            &mut state,
            config,
            restrictions,
            self_id,
            now_ns,
            &mut skipped_effects,
        )? {
            WithdrawalQueueOutcome::Ready(request) => Some(request),
            WithdrawalQueueOutcome::None | WithdrawalQueueOutcome::CoolingDown { .. } => None,
        }
    };

    let transition = complete_allocation(mem::take(&mut state.op_state), op_id, pending_req);
    let mut result = apply_transition_result(state, transition)?;
    skipped_effects.append(&mut result.effects);
    result.effects = skipped_effects;
    Ok(result)
}

#[cfg(any(feature = "action-sync-external", test))]
fn handle_sync_external_assets(
    mut state: VaultState,
    new_external_assets: u128,
    op_id: u64,
) -> Result<KernelResult, KernelError> {
    require_active_op_id(
        &state.op_state,
        op_id,
        InvalidStateCode::SyncExternalRequiresActiveOp,
    )?;

    ensure_sync_external_state_allowed(&state.op_state)?;
    let sync_plan = plan_external_asset_sync(&state, new_external_assets)?;

    state.external_assets = sync_plan.new_external_assets;
    state.total_assets = sync_plan.new_total_assets;

    let total_assets = state.total_assets;
    Ok(KernelResult::new(
        state,
        vec![KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::ExternalAssetsSynced {
                op_id,
                new_external_assets: sync_plan.new_external_assets,
                total_assets,
            },
        }],
    ))
}

#[cfg(any(feature = "action-sync-external", test))]
fn handle_rebalance_withdraw(
    mut state: VaultState,
    op_id: u64,
    amount: u128,
) -> Result<KernelResult, KernelError> {
    match &state.op_state {
        OpState::Idle => {}
        OpState::Allocating(_) => {
            require_active_op_id(
                &state.op_state,
                op_id,
                InvalidStateCode::SyncExternalRequiresActiveOp,
            )?;
        }
        _ => {
            return Err(KernelError::from(
                InvalidStateCode::RebalanceWithdrawRequiresIdle,
            ));
        }
    }

    if amount > state.external_assets {
        return Err(KernelError::from(
            InvalidStateCode::RebalanceWithdrawExceedsExternalAssets,
        ));
    }

    state.external_assets -= amount;
    state.idle_assets = state
        .idle_assets
        .checked_add(amount)
        .ok_or_else(|| KernelError::from(InvalidStateCode::RebalanceWithdrawOverflowsIdleAssets))?;
    state.sync_total_assets();

    let new_external_assets = state.external_assets;
    let total_assets = state.total_assets;
    Ok(KernelResult::new(
        state,
        vec![KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::ExternalAssetsSynced {
                op_id,
                new_external_assets,
                total_assets,
            },
        }],
    ))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_abort_refreshing(mut state: VaultState, op_id: u64) -> Result<KernelResult, KernelError> {
    require_active_op_id(
        &state.op_state,
        op_id,
        InvalidStateCode::AbortRefreshingRequiresActiveOp,
    )?;

    if !matches!(state.op_state, OpState::Refreshing(_)) {
        return Err(KernelError::from(
            InvalidStateCode::AbortRefreshingRequiresRefreshing,
        ));
    }

    state.op_state = OpState::Idle;
    Ok(KernelResult::new(state, vec![]))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_abort_allocating(mut state: VaultState, op_id: u64) -> Result<KernelResult, KernelError> {
    let alloc = match &state.op_state {
        OpState::Allocating(s) => s,
        _ => {
            return Err(KernelError::from(
                InvalidStateCode::AbortAllocatingRequiresAllocating,
            ))
        }
    };

    check_op_id(alloc.op_id, op_id)?;
    state.restore_to_idle(alloc.remaining);
    state.op_state = OpState::Idle;
    Ok(KernelResult::new(state, vec![]))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_abort_withdrawing(
    mut state: VaultState,
    self_id: &Address,
    op_id: u64,
) -> Result<KernelResult, KernelError> {
    let withdraw = match &state.op_state {
        OpState::Withdrawing(s) => s,
        _ => {
            return Err(KernelError::from(
                InvalidStateCode::AbortWithdrawingRequiresWithdrawing,
            ))
        }
    };

    check_op_id(withdraw.op_id, op_id)?;
    validate_queue_head(
        &state.withdraw_queue,
        withdraw.request_id,
        &withdraw.owner,
        &withdraw.receiver,
        withdraw.escrow_shares,
    )?;

    state.restore_to_idle(withdraw.collected);

    let result = map_transition_result(stop_withdrawal(
        mem::take(&mut state.op_state),
        op_id,
        *self_id,
    ))?;
    state.op_state = result.new_state;
    state.withdraw_queue.dequeue();
    Ok(KernelResult::new(state, result.effects))
}

#[inline]
fn plan_payout_settlement(
    payout: &PayoutState,
    outcome: PayoutOutcome,
) -> Result<PayoutSettlement, KernelError> {
    match outcome {
        PayoutOutcome::Success => {
            let burn_shares = payout.burn_shares;
            let refund_shares = payout
                .escrow_shares
                .checked_sub(payout.burn_shares)
                .ok_or_else(|| {
                    KernelError::from(InvalidStateCode::PayoutSuccessSettlementMismatch)
                })?;
            Ok(PayoutSettlement {
                burn_shares,
                refund_shares,
                completed_amount: payout.amount,
                success: true,
            })
        }
        PayoutOutcome::Failure => Ok(PayoutSettlement {
            burn_shares: 0,
            refund_shares: payout.escrow_shares,
            completed_amount: 0,
            success: false,
        }),
    }
}

fn apply_payout_settlement(
    state: &mut VaultState,
    payout: &PayoutState,
    settlement: PayoutSettlement,
    escrow_address: Address,
    effects: &mut Vec<KernelEffect>,
) -> Result<(), KernelError> {
    if settlement.burn_shares > 0 {
        effects.push(KernelEffect::BurnShares {
            owner: escrow_address,
            shares: settlement.burn_shares,
        });
        state.total_shares = state
            .total_shares
            .checked_sub(settlement.burn_shares)
            .ok_or_else(|| KernelError::from(InvalidStateCode::PayoutBurnExceedsTotalShares))?;
    }

    push_refund_shares(
        effects,
        escrow_address,
        payout.owner,
        settlement.refund_shares,
    );

    if settlement.success {
        state.idle_assets = state
            .idle_assets
            .checked_sub(payout.amount)
            .ok_or_else(|| KernelError::from(InvalidStateCode::PayoutFailureRestoreIdleMismatch))?;
        state.sync_total_assets();
    }

    state.op_state = OpState::Idle;
    Ok(())
}

/// Settle a payout after asset transfer attempt (success or failure).
fn handle_settle_payout(
    mut state: VaultState,
    self_id: &Address,
    op_id: u64,
    outcome: PayoutOutcome,
) -> Result<KernelResult, KernelError> {
    let payout = match mem::take(&mut state.op_state) {
        OpState::Payout(s) => s,
        _ => {
            return Err(KernelError::from(
                InvalidStateCode::SettlePayoutRequiresPayout,
            ))
        }
    };

    check_op_id(payout.op_id, op_id)?;

    validate_queue_head(
        &state.withdraw_queue,
        payout.request_id,
        &payout.owner,
        &payout.receiver,
        payout.escrow_shares,
    )?;

    let escrow_address = *self_id;
    let mut effects = Vec::new();

    let settlement = plan_payout_settlement(&payout, outcome)?;
    apply_payout_settlement(
        &mut state,
        &payout,
        settlement,
        escrow_address,
        &mut effects,
    )?;

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::PayoutCompleted {
            op_id,
            success: settlement.success,
            burn_shares: settlement.burn_shares,
            refund_shares: settlement.refund_shares,
            amount: settlement.completed_amount,
        },
    });

    state.withdraw_queue.dequeue();
    Ok(KernelResult::new(state, effects))
}

#[cfg(any(feature = "action-refresh-fees", test))]
fn handle_refresh_fees(
    mut state: VaultState,
    config: &VaultConfig,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::RefreshFeesRequiresIdle));
    }

    // Reject backwards time to prevent fee calculation issues
    if now_ns <= state.fee_anchor.timestamp_ns {
        return Err(KernelError::from(
            InvalidStateCode::FeeRefreshTimestampMustAdvance,
        ));
    }

    let cur_total_assets = state.total_assets;
    let mut total_supply = state.total_shares;
    let anchor = state.fee_anchor;
    let mut effects = Vec::new();

    // Cap effective total_assets for fee accrual (mitigates donation attacks)
    let fee_total_assets = total_assets_for_fee_accrual(
        cur_total_assets,
        anchor.total_assets,
        anchor.timestamp_ns.into(),
        now_ns.into(),
        config.fees.max_total_assets_growth_rate,
    );

    // Management fees (time-based, pro-rated over elapsed time)
    let mgmt_shares = compute_management_fee_shares(
        fee_total_assets,
        cur_total_assets,
        total_supply,
        config.fees.management.fee_wad,
        anchor.timestamp_ns.into(),
        now_ns.into(),
    );
    mint_fee_shares(
        &mut effects,
        &mut total_supply,
        mgmt_shares,
        config.fees.management.recipient,
    )?;

    // Performance fees (profit-based)
    let profit = fee_total_assets.saturating_sub(anchor.total_assets);
    let fee_assets = config
        .fees
        .performance
        .fee_wad
        .apply_floored(Number::from(profit));
    let perf_shares = compute_fee_shares_from_assets(
        fee_assets,
        Number::from(cur_total_assets),
        Number::from(total_supply),
    );
    mint_fee_shares(
        &mut effects,
        &mut total_supply,
        perf_shares,
        config.fees.performance.recipient,
    )?;

    state.total_shares = total_supply;
    state.fee_anchor = FeeAccrualAnchor::new(cur_total_assets, now_ns);

    effects.push(KernelEffect::EmitEvent {
        event: crate::effects::KernelEvent::FeesRefreshed {
            now_ns: now_ns.into(),
            total_assets: cur_total_assets,
        },
    });

    Ok(KernelResult::new(state, effects))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_emergency_reset(
    mut state: VaultState,
    self_id: &Address,
) -> Result<KernelResult, KernelError> {
    let prev_state = mem::take(&mut state.op_state);
    let from_code = prev_state.kind_code();
    let op_id = match prev_state.op_id() {
        Some(op_id) => op_id,
        None => {
            return Err(KernelError::from(
                InvalidStateCode::EmergencyResetAlreadyIdle,
            ))
        }
    };

    let mut effects = Vec::new();
    let escrow_address = *self_id;

    match prev_state {
        OpState::Idle => {
            return Err(KernelError::from(
                InvalidStateCode::EmergencyResetAlreadyIdle,
            ))
        }
        OpState::Refreshing(_) => {
            // No assets in-flight, just reset.
        }
        OpState::Allocating(alloc) => {
            // Restore unallocated assets back to idle.
            state.restore_to_idle(alloc.remaining);
        }
        OpState::Withdrawing(w) => {
            push_refund_shares(&mut effects, escrow_address, w.owner, w.escrow_shares);
            // Restore any collected assets back to idle.
            state.restore_to_idle(w.collected);
            state.withdraw_queue.dequeue();
        }
        OpState::Payout(p) => {
            push_refund_shares(&mut effects, escrow_address, p.owner, p.escrow_shares);
            // Restore payout amount back to idle.
            state.restore_to_idle(p.amount);
            state.withdraw_queue.dequeue();
        }
    }

    state.op_state = OpState::Idle;
    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::EmergencyResetCompleted {
            op_id,
            from_state: from_code,
        },
    });

    Ok(KernelResult::new(state, effects))
}

/// Apply a kernel action to state, returning updated state and effects.
#[allow(unused_mut)]
pub fn apply_action(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    action: KernelAction,
) -> Result<KernelResult, KernelError> {
    dispatch::apply_action(state, config, restrictions, self_id, action)
}

fn enforce_restrictions(
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    actor: &Address,
) -> Result<(), KernelError> {
    access::enforce_restrictions(config, restrictions, self_id, actor)
}

#[inline]
fn is_globally_paused(config: &VaultConfig, restrictions: Option<&Restrictions>) -> bool {
    let _ = restrictions;
    config.paused
}

mod planning {
    use super::*;

    pub(super) fn plan_idle_payout(
        state: &VaultState,
    ) -> Result<Option<IdlePayoutPlan>, KernelError> {
        let (request_owner, request_receiver, request_escrow, request_expected) = state
            .withdraw_queue
            .head()
            .map(|(_, request)| {
                (
                    request.owner,
                    request.receiver,
                    request.escrow_shares,
                    request.expected_assets,
                )
            })
            .ok_or_else(|| KernelError::from(InvalidStateCode::UnexpectedEmptyQueue))?;

        let withdrawing = match &state.op_state {
            OpState::Withdrawing(withdrawing) => withdrawing,
            _ => {
                return Err(KernelError::from(
                    InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks,
                ))
            }
        };

        if request_owner != withdrawing.owner
            || request_receiver != withdrawing.receiver
            || request_escrow != withdrawing.escrow_shares
        {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalQueueHeadMismatch,
            ));
        }

        let available_assets = state.idle_assets;
        if available_assets < request_expected
            && available_assets < crate::state::queue::MIN_WITHDRAWAL_ASSETS
        {
            return Ok(None);
        }

        let Some(settlement) =
            compute_idle_settlement(request_escrow, request_expected, available_assets)
        else {
            return Ok(None);
        };

        if settlement.assets_out == 0 {
            return Ok(None);
        }

        Ok(Some(IdlePayoutPlan {
            op_id: withdrawing.op_id,
            request_id: withdrawing.request_id,
            receiver: withdrawing.receiver,
            assets_out: settlement.assets_out,
            burn_shares: settlement.settlement.to_burn,
        }))
    }
}

mod conversions {
    use super::*;

    pub(super) fn effective_totals(state: &VaultState, config: &VaultConfig) -> EffectiveTotals {
        EffectiveTotals {
            supply: state
                .total_shares
                .saturating_add(config.virtual_shares.max(1)),
            assets: state
                .total_assets
                .saturating_add(config.virtual_assets.max(1)),
        }
    }

    pub(super) fn convert_to_shares(
        state: &VaultState,
        config: &VaultConfig,
        assets: u128,
    ) -> u128 {
        let t = effective_totals(state, config);
        u128::from(mul_div_floor(
            Number::from(assets),
            Number::from(t.supply),
            Number::from(t.assets),
        ))
    }

    pub(super) fn convert_to_assets(
        state: &VaultState,
        config: &VaultConfig,
        shares: u128,
    ) -> u128 {
        let t = effective_totals(state, config);
        u128::from(mul_div_floor(
            Number::from(shares),
            Number::from(t.assets),
            Number::from(t.supply),
        ))
    }

    pub(super) fn convert_to_shares_ceil(
        state: &VaultState,
        config: &VaultConfig,
        assets: u128,
    ) -> u128 {
        let t = effective_totals(state, config);
        u128::from(mul_div_ceil(
            Number::from(assets),
            Number::from(t.supply),
            Number::from(t.assets),
        ))
    }

    pub(super) fn convert_to_assets_ceil(
        state: &VaultState,
        config: &VaultConfig,
        shares: u128,
    ) -> u128 {
        let t = effective_totals(state, config);
        u128::from(mul_div_ceil(
            Number::from(shares),
            Number::from(t.assets),
            Number::from(t.supply),
        ))
    }
}

mod access {
    use super::*;

    pub(super) fn enforce_restrictions(
        config: &VaultConfig,
        restrictions: Option<&Restrictions>,
        _self_id: &Address,
        actor: &Address,
    ) -> Result<(), KernelError> {
        if config.paused {
            return Err(KernelError::Restricted(RestrictionKind::Paused));
        }
        if let Some(restrictions) = restrictions {
            if let Some(kind) = restrictions.is_restricted(actor) {
                return Err(KernelError::Restricted(kind));
            }
        }
        Ok(())
    }
}

mod dispatch {
    use super::*;

    #[allow(unused_mut)]
    pub(super) fn apply_action(
        mut state: VaultState,
        config: &VaultConfig,
        restrictions: Option<&Restrictions>,
        self_id: &Address,
        action: KernelAction,
    ) -> Result<KernelResult, KernelError> {
        match action {
            KernelAction::Deposit {
                owner,
                receiver,
                assets_in,
                min_shares_out,
                now_ns: _,
            } => handle_deposit(
                state,
                config,
                restrictions,
                self_id,
                owner,
                receiver,
                assets_in,
                min_shares_out,
            ),

            KernelAction::AtomicWithdraw {
                owner,
                receiver,
                operator,
                assets_out,
                max_shares_burned,
                now_ns: _,
            } => handle_atomic_withdraw(
                state,
                config,
                restrictions,
                self_id,
                owner,
                receiver,
                operator,
                assets_out,
                max_shares_burned,
            ),

            KernelAction::AtomicRedeem {
                owner,
                receiver,
                operator,
                shares,
                min_assets_out,
                now_ns: _,
            } => handle_atomic_redeem(
                state,
                config,
                restrictions,
                self_id,
                owner,
                receiver,
                operator,
                shares,
                min_assets_out,
            ),

            KernelAction::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out,
                now_ns,
            } => handle_request_withdraw(
                state,
                config,
                restrictions,
                self_id,
                owner,
                receiver,
                shares,
                min_assets_out,
                now_ns,
            ),

            KernelAction::ExecuteWithdraw { now_ns } => {
                handle_execute_withdraw(state, config, restrictions, self_id, now_ns)
            }

            #[cfg(any(feature = "action-allocation-lifecycle", test))]
            KernelAction::BeginAllocating { op_id, plan, .. } => {
                handle_begin_allocating(state, op_id, plan)
            }
            #[cfg(not(any(feature = "action-allocation-lifecycle", test)))]
            KernelAction::BeginAllocating { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-allocation-lifecycle", test))]
            KernelAction::FinishAllocating { op_id, now_ns } => {
                handle_finish_allocating(state, config, restrictions, self_id, op_id, now_ns)
            }
            #[cfg(not(any(feature = "action-allocation-lifecycle", test)))]
            KernelAction::FinishAllocating { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-refresh-lifecycle", test))]
            KernelAction::BeginRefreshing { op_id, plan, .. } => {
                let transition = start_refresh(mem::take(&mut state.op_state), plan, op_id);
                apply_transition_result(state, transition)
            }
            #[cfg(not(any(feature = "action-refresh-lifecycle", test)))]
            KernelAction::BeginRefreshing { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-refresh-lifecycle", test))]
            KernelAction::FinishRefreshing { op_id, .. } => {
                let transition = complete_refresh(mem::take(&mut state.op_state), op_id);
                apply_transition_result(state, transition)
            }
            #[cfg(not(any(feature = "action-refresh-lifecycle", test)))]
            KernelAction::FinishRefreshing { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-sync-external", test))]
            KernelAction::SyncExternalAssets {
                new_external_assets,
                op_id,
                ..
            } => handle_sync_external_assets(state, new_external_assets, op_id),
            #[cfg(not(any(feature = "action-sync-external", test)))]
            KernelAction::SyncExternalAssets { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-sync-external", test))]
            KernelAction::RebalanceWithdraw { op_id, amount, .. } => {
                handle_rebalance_withdraw(state, op_id, amount)
            }
            #[cfg(not(any(feature = "action-sync-external", test)))]
            KernelAction::RebalanceWithdraw { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-recovery", test))]
            KernelAction::AbortRefreshing { op_id } => handle_abort_refreshing(state, op_id),
            #[cfg(not(any(feature = "action-recovery", test)))]
            KernelAction::AbortRefreshing { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-recovery", test))]
            KernelAction::AbortAllocating { op_id } => handle_abort_allocating(state, op_id),
            #[cfg(not(any(feature = "action-recovery", test)))]
            KernelAction::AbortAllocating { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-recovery", test))]
            KernelAction::AbortWithdrawing { op_id } => {
                handle_abort_withdrawing(state, self_id, op_id)
            }
            #[cfg(not(any(feature = "action-recovery", test)))]
            KernelAction::AbortWithdrawing { .. } => Err(KernelError::NotImplemented),

            KernelAction::SettlePayout { op_id, outcome } => {
                handle_settle_payout(state, self_id, op_id, outcome)
            }

            #[cfg(any(feature = "action-pause", test))]
            KernelAction::Pause { paused } => Ok(KernelResult::new(
                state,
                vec![KernelEffect::EmitEvent {
                    event: crate::effects::KernelEvent::PauseUpdated { paused },
                }],
            )),
            #[cfg(not(any(feature = "action-pause", test)))]
            KernelAction::Pause { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-refresh-fees", test))]
            KernelAction::RefreshFees { now_ns } => handle_refresh_fees(state, config, now_ns),
            #[cfg(not(any(feature = "action-refresh-fees", test)))]
            KernelAction::RefreshFees { .. } => Err(KernelError::NotImplemented),

            #[cfg(any(feature = "action-recovery", test))]
            KernelAction::EmergencyReset => handle_emergency_reset(state, self_id),
            #[cfg(not(any(feature = "action-recovery", test)))]
            KernelAction::EmergencyReset => Err(KernelError::NotImplemented),
        }
    }
}

// Tests

#[cfg(test)]
mod tests;
