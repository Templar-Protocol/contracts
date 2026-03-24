//! Kernel action dispatch for vault state transitions.
//!
//! This module defines the public `KernelAction` enum and a dispatcher that
//! applies actions to `VaultState` and returns effects.

extern crate alloc;

use core::mem;

use crate::effects::{KernelEffect, KernelEvent, WithdrawalSkipReason};
use crate::error::{InvalidConfigCode, InvalidStateCode, KernelError};
use crate::math::number::Number;
#[cfg(any(feature = "action-refresh-fees", test))]
use crate::math::wad::compute_fee_shares_from_assets;
#[cfg(any(feature = "action-refresh-fees", test))]
use crate::math::wad::{compute_management_fee_shares, total_assets_for_fee_accrual};
use crate::math::wad::{mul_div_ceil, mul_div_floor};
use crate::restrictions::{RestrictionKind, Restrictions};
use crate::state::op_state::{OpState, TargetId};
use crate::state::queue::{is_past_cooldown, QueueError, WithdrawQueue};
#[cfg(any(feature = "action-refresh-fees", test))]
use crate::state::vault::FeeAccrualAnchor;
use crate::state::vault::{VaultConfig, VaultState};
#[cfg(any(feature = "action-recovery", test))]
use crate::transitions::stop_withdrawal;
use crate::transitions::TransitionResult;
#[cfg(any(feature = "action-allocation-lifecycle", test))]
use crate::transitions::{complete_allocation, start_allocation};
#[cfg(any(feature = "action-refresh-lifecycle", test))]
use crate::transitions::{complete_refresh, start_refresh};
use crate::transitions::{start_withdrawal, TransitionError, WithdrawalRequest};
use crate::types::{Address, TimestampNs};
use alloc::vec;
use alloc::vec::Vec;
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
    Success {
        burn_shares: u128,
        refund_shares: u128,
    },
    Failure {
        restore_idle: u128,
        refund_shares: u128,
    },
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
        plan: Vec<(TargetId, u128)>,
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
        amount: u128,
        kind: AtomicPayoutKind,
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
    AbortAllocating { op_id: u64, restore_idle: u128 },

    /// Abort a withdrawal operation (e.g., on external call failure).
    ///
    /// Transition: Withdrawing -> Idle
    AbortWithdrawing { op_id: u64, refund_shares: u128 },

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
    pub fn begin_allocating(op_id: u64, plan: Vec<(TargetId, u128)>, now_ns: TimestampNs) -> Self {
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
        now_ns: TimestampNs,
    ) -> Self {
        Self::AtomicWithdraw {
            owner,
            receiver,
            operator,
            amount: assets_out,
            kind: AtomicPayoutKind::Withdraw,
            now_ns,
        }
    }

    #[must_use]
    pub fn atomic_redeem(
        owner: Address,
        receiver: Address,
        operator: Address,
        shares: u128,
        now_ns: TimestampNs,
    ) -> Self {
        Self::AtomicWithdraw {
            owner,
            receiver,
            operator,
            amount: shares,
            kind: AtomicPayoutKind::Redeem,
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
    pub fn abort_allocating(op_id: u64, restore_idle: u128) -> Self {
        Self::AbortAllocating {
            op_id,
            restore_idle,
        }
    }

    #[must_use]
    pub fn abort_withdrawing(op_id: u64, refund_shares: u128) -> Self {
        Self::AbortWithdrawing {
            op_id,
            refund_shares,
        }
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
            | Self::FinishRefreshing { op_id, .. }
            | Self::AbortRefreshing { op_id }
            | Self::SettlePayout { op_id, .. }
            | Self::AbortAllocating { op_id, .. }
            | Self::AbortWithdrawing { op_id, .. } => Some(*op_id),
            Self::Deposit { .. }
            | Self::AtomicWithdraw { .. }
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
            | Self::RequestWithdraw { now_ns, .. }
            | Self::ExecuteWithdraw { now_ns }
            | Self::BeginRefreshing { now_ns, .. }
            | Self::FinishAllocating { now_ns, .. }
            | Self::SyncExternalAssets { now_ns, .. }
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

#[templar_vault_macros::vault_derive(borsh, serde, postcard)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtomicPayoutKind {
    Withdraw,
    Redeem,
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
    EffectiveTotals {
        supply: state
            .total_shares
            .saturating_add(config.virtual_shares.max(1)),
        assets: state
            .total_assets
            .saturating_add(config.virtual_assets.max(1)),
    }
}

/// Convert an asset amount to shares (floor rounding — fewer shares, favors vault).
pub fn convert_to_shares(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    let t = effective_totals(state, config);
    u128::from(mul_div_floor(
        Number::from(assets),
        Number::from(t.supply),
        Number::from(t.assets),
    ))
}

/// Convert a share amount to assets (floor rounding — fewer assets, favors vault).
pub fn convert_to_assets(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    let t = effective_totals(state, config);
    u128::from(mul_div_floor(
        Number::from(shares),
        Number::from(t.assets),
        Number::from(t.supply),
    ))
}

/// Convert an asset amount to shares (ceil rounding — more shares, favors user).
///
/// Used by ERC-4626 `preview_withdraw` to compute shares burned (rounds against user).
pub fn convert_to_shares_ceil(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    let t = effective_totals(state, config);
    u128::from(mul_div_ceil(
        Number::from(assets),
        Number::from(t.supply),
        Number::from(t.assets),
    ))
}

/// Convert a share amount to assets (ceil rounding — more assets, favors user).
///
/// Used by ERC-4626 `preview_mint` to compute assets needed (rounds against user).
pub fn convert_to_assets_ceil(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    let t = effective_totals(state, config);
    u128::from(mul_div_ceil(
        Number::from(shares),
        Number::from(t.assets),
        Number::from(t.supply),
    ))
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
        None => return Err(KernelError::invalid_state_code(error_code)),
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
    owner: &Address,
    receiver: &Address,
    escrow_shares: u128,
) -> Result<(), KernelError> {
    let Some((_, pending)) = queue.head() else {
        return Err(KernelError::EmptyQueue);
    };
    if pending.owner != *owner
        || pending.receiver != *receiver
        || pending.escrow_shares != escrow_shares
    {
        return Err(KernelError::invalid_state_code(
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
        *total_supply = total_supply.checked_add(minted).ok_or_else(|| {
            KernelError::invalid_state_code(InvalidStateCode::FeeMintOverflowTotalSupply)
        })?;
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
            KernelError::invalid_state_code(InvalidStateCode::WithdrawalQueueCacheOverflow)
        }
        QueueError::WithdrawalNotFound { .. } => {
            KernelError::invalid_state_code(InvalidStateCode::WithdrawalQueueMissingEntry)
        }
        QueueError::QueueEmpty => {
            KernelError::invalid_state_code(InvalidStateCode::WithdrawalQueueEmpty)
        }
        QueueError::InvariantViolation { .. } => {
            KernelError::invalid_state_code(InvalidStateCode::WithdrawalQueueInvariantViolation)
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
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::DepositRequiresIdle,
        ));
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

    state.total_assets = state.total_assets.checked_add(assets_in).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::DepositOverflowTotalAssets)
    })?;
    state.idle_assets = state.idle_assets.checked_add(assets_in).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::DepositOverflowIdleAssets)
    })?;
    state.total_shares = state.total_shares.checked_add(shares_out).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::MintOverflowTotalShares)
    })?;

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

fn handle_atomic_withdraw(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: Address,
    receiver: Address,
    operator: Address,
    amount: u128,
    kind: AtomicPayoutKind,
) -> Result<KernelResult, KernelError> {
    enforce_restrictions(config, restrictions, self_id, &owner)?;
    enforce_restrictions(config, restrictions, self_id, &receiver)?;
    if !state.is_idle() {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::AtomicWithdrawRequiresIdle,
        ));
    }
    if amount == 0 {
        return Err(KernelError::ZeroAmount);
    }

    let (shares, assets_out) = match kind {
        AtomicPayoutKind::Withdraw => {
            if amount > state.idle_assets {
                return Err(KernelError::invalid_state_code(
                    InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
                ));
            }
            (convert_to_shares_ceil(&state, config, amount), amount)
        }
        AtomicPayoutKind::Redeem => {
            let assets_out = convert_to_assets(&state, config, amount);
            if assets_out > state.idle_assets {
                return Err(KernelError::invalid_state_code(
                    InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
                ));
            }
            (amount, assets_out)
        }
    };

    if assets_out > state.idle_assets {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
        ));
    }
    state.total_shares = state.total_shares.checked_sub(shares).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::AtomicWithdrawBurnExceedsTotalShares)
    })?;
    state.idle_assets = state.idle_assets.checked_sub(assets_out).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::AtomicWithdrawExceedsIdleAssets)
    })?;
    state.total_assets = state.total_assets.checked_sub(assets_out).ok_or_else(|| {
        KernelError::invalid_state_code(InvalidStateCode::AtomicWithdrawTotalAssetsUnderflow)
    })?;

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
    enforce_restrictions(config, restrictions, self_id, &owner)?;
    enforce_restrictions(config, restrictions, self_id, &receiver)?;
    if !state.is_idle() {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::RequestWithdrawRequiresIdle,
        ));
    }
    if shares == 0 {
        return Err(KernelError::ZeroAmount);
    }

    let expected_assets = convert_to_assets(&state, config, shares);
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

    let id = state
        .withdraw_queue
        .enqueue(
            owner,
            receiver,
            shares,
            expected_assets,
            now_ns,
            config.max_pending_withdrawals,
        )
        .map_err(map_queue_error)?;

    let effects = vec![
        KernelEffect::TransferShares {
            from: owner,
            to: *self_id,
            shares,
        },
        KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::WithdrawalRequested {
                id,
                owner,
                receiver,
                shares,
                expected_assets,
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
        return Err(KernelError::invalid_state_code(error_code));
    }

    if config.paused {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }
    if matches!(restrictions, Some(Restrictions::Paused)) {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }

    let mut skipped_effects = Vec::new();

    loop {
        let Some((_, pending_ref)) = state.withdraw_queue.head() else {
            return if skipped_effects.is_empty() {
                Err(KernelError::EmptyQueue)
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            };
        };
        let pending_owner = pending_ref.owner;
        let pending_receiver = pending_ref.receiver;
        let pending_escrow_shares = pending_ref.escrow_shares;
        let pending_expected_assets = pending_ref.expected_assets;
        let pending_requested_at_ns = pending_ref.requested_at_ns;

        let actor_restricted = restrictions
            .and_then(|r| r.is_restricted(&pending_owner, self_id))
            .or_else(|| restrictions.and_then(|r| r.is_restricted(&pending_receiver, self_id)));

        if pending_expected_assets == 0 || actor_restricted.is_some() {
            let (pending_id, pending) = match state.withdraw_queue.dequeue() {
                Some(entry) => entry,
                None => return Err(KernelError::EmptyQueue),
            };
            push_refund_shares(
                &mut skipped_effects,
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
                    reason: if actor_restricted.is_some() {
                        WithdrawalSkipReason::Restricted
                    } else {
                        WithdrawalSkipReason::ZeroExpectedAssets
                    },
                },
            });
            continue;
        }

        if !is_past_cooldown(
            pending_requested_at_ns,
            now_ns,
            config.withdrawal_cooldown_ns,
        ) {
            return if skipped_effects.is_empty() {
                Err(KernelError::Cooldown {
                    requested_at: pending_requested_at_ns,
                    now: now_ns,
                    cooldown_ns: config.withdrawal_cooldown_ns,
                })
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            };
        }

        let op_id = state.allocate_op_id();
        let request = WithdrawalRequest {
            op_id,
            amount: pending_expected_assets,
            receiver: pending_receiver,
            owner: pending_owner,
            escrow_shares: pending_escrow_shares,
        };

        let transition = start_withdrawal(mem::take(&mut state.op_state), request);
        let mut result = apply_transition_result(state, transition)?;
        skipped_effects.append(&mut result.effects);
        result.effects = skipped_effects;
        return Ok(result);
    }
}

/// Start an allocation: transition to Allocating and decrement idle assets.
#[cfg(any(feature = "action-allocation-lifecycle", test))]
fn handle_begin_allocating(
    mut state: VaultState,
    op_id: u64,
    plan: Vec<(TargetId, u128)>,
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
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::StartAllocationMustReturnAllocating,
            ))
        }
    };

    if alloc_total > state.idle_assets {
        return Err(KernelError::invalid_state_code(
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

    let pending_req = if config.paused {
        None
    } else {
        loop {
            let Some((_, pending)) = state.withdraw_queue.head() else {
                break None;
            };

            let owner = pending.owner;
            let receiver = pending.receiver;
            let escrow_shares = pending.escrow_shares;
            let expected_assets = pending.expected_assets;
            let requested_at_ns = pending.requested_at_ns;

            if !is_past_cooldown(requested_at_ns, now_ns, config.withdrawal_cooldown_ns) {
                break None;
            }

            let actor_restricted = restrictions
                .and_then(|r| r.is_restricted(&owner, self_id))
                .or_else(|| restrictions.and_then(|r| r.is_restricted(&receiver, self_id)));

            if expected_assets == 0 || actor_restricted.is_some() {
                let (pending_id, skipped) = state
                    .withdraw_queue
                    .dequeue()
                    .ok_or(KernelError::EmptyQueue)?;
                push_refund_shares(
                    &mut skipped_effects,
                    *self_id,
                    skipped.owner,
                    skipped.escrow_shares,
                );
                skipped_effects.push(KernelEffect::EmitEvent {
                    event: KernelEvent::WithdrawalSkipped {
                        id: pending_id,
                        owner: skipped.owner,
                        receiver: skipped.receiver,
                        escrow_shares: skipped.escrow_shares,
                        expected_assets: skipped.expected_assets,
                        reason: if actor_restricted.is_some() {
                            WithdrawalSkipReason::Restricted
                        } else {
                            WithdrawalSkipReason::ZeroExpectedAssets
                        },
                    },
                });
                continue;
            }

            break Some(WithdrawalRequest {
                op_id: state.allocate_op_id(),
                amount: expected_assets,
                receiver,
                owner,
                escrow_shares,
            });
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

    match state.op_state {
        OpState::Allocating(_) | OpState::Withdrawing(_) | OpState::Refreshing(_) => {}
        _ => {
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::SyncExternalRequiresAllowedStates,
            ));
        }
    }

    // Overflow protection: idle_assets + new_external must fit in u128.
    let new_total = state
        .idle_assets
        .checked_add(new_external_assets)
        .ok_or_else(|| {
            KernelError::invalid_state_code(InvalidStateCode::SyncExternalOverflowIdlePlusExternal)
        })?;

    // Sanity bound: prevent a compromised allocator from inflating
    // total_assets beyond 2x the previous value. During allocation,
    // assets are "in flight" (decremented from idle, not yet synced
    // to external) so we include the remaining allocation amount in
    // the reference total for the bound check.
    let in_flight = match &state.op_state {
        OpState::Allocating(s) => s.remaining,
        _ => 0,
    };
    let reference_total = state.total_assets.saturating_add(in_flight);
    if reference_total > 0 && new_total > reference_total.saturating_mul(2) {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::SyncExternalWouldMoreThanDoubleTotalAssets,
        ));
    }

    state.external_assets = new_external_assets;
    state.total_assets = new_total;

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
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::AbortRefreshingRequiresRefreshing,
        ));
    }

    state.op_state = OpState::Idle;
    Ok(KernelResult::new(state, vec![]))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_abort_allocating(
    mut state: VaultState,
    op_id: u64,
    restore_idle: u128,
) -> Result<KernelResult, KernelError> {
    let alloc = match &state.op_state {
        OpState::Allocating(s) => s,
        _ => {
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::AbortAllocatingRequiresAllocating,
            ))
        }
    };

    check_op_id(alloc.op_id, op_id)?;
    if restore_idle != alloc.remaining {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::AbortAllocatingRestoreIdleMismatch,
        ));
    }

    state.restore_to_idle(restore_idle);
    state.op_state = OpState::Idle;
    Ok(KernelResult::new(state, vec![]))
}

#[cfg(any(feature = "action-recovery", test))]
fn handle_abort_withdrawing(
    mut state: VaultState,
    self_id: &Address,
    op_id: u64,
    refund_shares: u128,
) -> Result<KernelResult, KernelError> {
    let withdraw = match &state.op_state {
        OpState::Withdrawing(s) => s,
        _ => {
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::AbortWithdrawingRequiresWithdrawing,
            ))
        }
    };

    check_op_id(withdraw.op_id, op_id)?;
    if refund_shares != withdraw.escrow_shares {
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::AbortWithdrawingRefundMismatch,
        ));
    }

    validate_queue_head(
        &state.withdraw_queue,
        &withdraw.owner,
        &withdraw.receiver,
        withdraw.escrow_shares,
    )?;

    let result = map_transition_result(stop_withdrawal(
        mem::take(&mut state.op_state),
        op_id,
        *self_id,
    ))?;
    state.op_state = result.new_state;
    state.withdraw_queue.dequeue();
    Ok(KernelResult::new(state, result.effects))
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
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::SettlePayoutRequiresPayout,
            ))
        }
    };

    check_op_id(payout.op_id, op_id)?;

    validate_queue_head(
        &state.withdraw_queue,
        &payout.owner,
        &payout.receiver,
        payout.escrow_shares,
    )?;

    let escrow_address = *self_id;
    let mut effects = Vec::new();

    let (burn_shares, refund_shares, amount, success) = match outcome {
        PayoutOutcome::Success {
            burn_shares: burn_amount,
            refund_shares: refund_amount,
        } => {
            let settled_shares = burn_amount.checked_add(refund_amount).ok_or_else(|| {
                KernelError::invalid_state_code(InvalidStateCode::PayoutSuccessSettlementMismatch)
            })?;

            if settled_shares != payout.escrow_shares {
                return Err(KernelError::invalid_state_code(
                    InvalidStateCode::PayoutSuccessSettlementMismatch,
                ));
            }

            if burn_amount > 0 {
                effects.push(KernelEffect::BurnShares {
                    owner: escrow_address,
                    shares: burn_amount,
                });
                state.total_shares =
                    state.total_shares.checked_sub(burn_amount).ok_or_else(|| {
                        KernelError::invalid_state_code(
                            InvalidStateCode::PayoutBurnExceedsTotalShares,
                        )
                    })?;
            }
            push_refund_shares(&mut effects, escrow_address, payout.owner, refund_amount);

            state.op_state = OpState::Idle;
            (burn_amount, refund_amount, payout.amount, true)
        }
        PayoutOutcome::Failure {
            restore_idle,
            refund_shares: refund_amount,
        } => {
            if refund_amount != payout.escrow_shares {
                return Err(KernelError::invalid_state_code(
                    InvalidStateCode::PayoutFailureSettlementMismatch,
                ));
            }
            if restore_idle != payout.amount {
                return Err(KernelError::invalid_state_code(
                    InvalidStateCode::PayoutFailureRestoreIdleMismatch,
                ));
            }

            push_refund_shares(&mut effects, escrow_address, payout.owner, refund_amount);

            state.restore_to_idle(restore_idle);
            state.op_state = OpState::Idle;
            (0, refund_amount, 0, false)
        }
    };

    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::PayoutCompleted {
            op_id,
            success,
            burn_shares,
            refund_shares,
            amount,
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
        return Err(KernelError::invalid_state_code(
            InvalidStateCode::RefreshFeesRequiresIdle,
        ));
    }

    // Reject backwards time to prevent fee calculation issues
    if now_ns <= state.fee_anchor.timestamp_ns {
        return Err(KernelError::invalid_state_code(
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
        anchor.timestamp_ns,
        now_ns,
        config.fees.max_total_assets_growth_rate,
    );

    // Management fees (time-based, pro-rated over elapsed time)
    let mgmt_shares = compute_management_fee_shares(
        fee_total_assets,
        cur_total_assets,
        total_supply,
        config.fees.management.fee_wad,
        anchor.timestamp_ns,
        now_ns,
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
            now_ns,
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
            return Err(KernelError::invalid_state_code(
                InvalidStateCode::EmergencyResetAlreadyIdle,
            ))
        }
    };

    let mut effects = Vec::new();
    let escrow_address = *self_id;

    match prev_state {
        OpState::Idle => {
            return Err(KernelError::invalid_state_code(
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
    if !config.is_max_pending_valid() {
        return Err(KernelError::invalid_config_code(
            InvalidConfigCode::MaxPendingWithdrawalsExceedsLimit,
        ));
    }

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
            amount,
            kind,
            now_ns: _,
        } => handle_atomic_withdraw(
            state,
            config,
            restrictions,
            self_id,
            owner,
            receiver,
            operator,
            amount,
            kind,
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

        #[cfg(any(feature = "action-recovery", test))]
        KernelAction::AbortRefreshing { op_id } => handle_abort_refreshing(state, op_id),
        #[cfg(not(any(feature = "action-recovery", test)))]
        KernelAction::AbortRefreshing { .. } => Err(KernelError::NotImplemented),

        #[cfg(any(feature = "action-recovery", test))]
        KernelAction::AbortAllocating {
            op_id,
            restore_idle,
        } => handle_abort_allocating(state, op_id, restore_idle),
        #[cfg(not(any(feature = "action-recovery", test)))]
        KernelAction::AbortAllocating { .. } => Err(KernelError::NotImplemented),

        #[cfg(any(feature = "action-recovery", test))]
        KernelAction::AbortWithdrawing {
            op_id,
            refund_shares,
        } => handle_abort_withdrawing(state, self_id, op_id, refund_shares),
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

fn enforce_restrictions(
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    actor: &Address,
) -> Result<(), KernelError> {
    if config.paused {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }
    if let Some(restrictions) = restrictions {
        if let Some(kind) = restrictions.is_restricted(actor, self_id) {
            return Err(KernelError::Restricted(kind));
        }
    }
    Ok(())
}

// Tests

#[cfg(test)]
mod tests;
