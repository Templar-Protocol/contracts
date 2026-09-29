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

#[cfg(any(
    feature = "action-immediate-deposit",
    feature = "action-epoch-settlement",
    feature = "action-refresh-fees",
    feature = "action-recovery",
    test
))]
use crate::state::vault::FeeAccrualAnchor;

#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::MIN_WITHDRAWAL_ASSETS;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::queue::settled_claim;
#[cfg(feature = "action-epoch-settlement")]
use crate::state::settlement::{EpochId, EpochState, SettlementRejection, ValuationReportRef};

use alloc::vec;
use alloc::vec::Vec;

#[cfg(any(feature = "action-refresh-fees", test))]
use crate::math::wad::{
    compute_fee_shares_from_assets, compute_management_fee_shares, total_assets_for_fee_accrual,
};
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
    pub owner: Address,
    pub receiver: Address,
    pub assets_out: u128,
    pub burn_shares: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
enum WithdrawalQueueOutcome {
    None,
    CoolingDown { requested_at_ns: TimestampNs },
    InsufficientLiquidity,
    #[cfg(feature = "action-epoch-settlement")]
    HeadNotSettled,
    #[cfg(feature = "action-epoch-settlement")]
    BelowMinAssetsOut { min: u128, claim: u128 },
    Ready(WithdrawalRequest),
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingWithdrawalHead {
    pub(crate) id: u64,
    pub(crate) owner: Address,
    pub(crate) receiver: Address,
    pub(crate) escrow_shares: u128,
    #[cfg(feature = "action-epoch-settlement")]
    pub(crate) min_assets_out: u128,
    #[cfg(not(feature = "action-epoch-settlement"))]
    pub(crate) expected_assets: u128,
    pub(crate) requested_at_ns: TimestampNs,
    #[cfg(feature = "action-epoch-settlement")]
    pub(crate) epoch_id: EpochId,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
enum WithdrawalHeadOutcome {
    Skip(WithdrawalSkipReason),
    CoolingDown { requested_at_ns: TimestampNs },
    #[cfg(feature = "action-epoch-settlement")]
    HeadNotSettled,
    #[cfg(feature = "action-epoch-settlement")]
    BelowMinAssetsOut { min: u128, claim: u128 },
    InsufficientLiquidity,
    #[cfg(feature = "action-epoch-settlement")]
    Ready { claim: u128 },
    #[cfg(not(feature = "action-epoch-settlement"))]
    Ready,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PayoutSettlement {
    pub(crate) burn_shares: u128,
    pub(crate) refund_shares: u128,
    pub(crate) completed_amount: u128,
    pub(crate) success: bool,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
#[cfg(any(feature = "action-recovery", test))]
pub(crate) struct EmergencyResetOutcome {
    pub(crate) state: VaultState,
    pub(crate) op_id: u64,
    pub(crate) from_code: u32,
    pub(crate) refund_owner: Option<Address>,
    pub(crate) refund_shares: u128,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct WithdrawalRequestPlan {
    pub(crate) owner: Address,
    pub(crate) receiver: Address,
    pub(crate) shares: u128,
    #[cfg(feature = "action-epoch-settlement")]
    pub(crate) min_assets_out: u128,
    #[cfg(not(feature = "action-epoch-settlement"))]
    pub(crate) expected_assets: u128,
    #[cfg(feature = "action-epoch-settlement")]
    pub(crate) epoch_id: EpochId,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "action-sync-external", test))]
struct ExternalAssetSyncPlan {
    new_external_assets: u128,
    new_total_assets: u128,
}

/// Plan an idle-funded payout from the current vault state.
///
/// Returns an error when the current idle liquidity cannot produce an actionable
/// payout for the queue head.
pub fn plan_idle_payout(
    state: &VaultState,
    min_withdrawal_assets: u128,
) -> Result<IdlePayoutPlan, KernelError> {
    #[cfg(feature = "action-epoch-settlement")]
    let planned = { planning::plan_idle_payout(state, min_withdrawal_assets) };
    #[cfg(not(feature = "action-epoch-settlement"))]
    let planned = { legacy_planning::plan_idle_payout(state, min_withdrawal_assets) };
    planned
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

    /// Close intake for the current epoch at `cutoff_ns`.
    ///
    /// Public keeper action (Idle-only). Authorization is enforced by the
    /// runtime adapter before dispatch.
    #[cfg(feature = "action-epoch-settlement")]
    BeginEpochCutoff {
        cutoff_ns: TimestampNs,
        now_ns: TimestampNs,
    },
    #[cfg(feature = "action-epoch-settlement")]
    SettleEpoch {
        report: ValuationReportRef,
        new_external_assets: u128,
        max_report_age_ns: u64,
        settle_now_ns: TimestampNs,
    },
    #[cfg(feature = "action-epoch-settlement")]
    CancelPendingWithdrawal {
        caller: Address,
        request_id: u64,
        now_ns: TimestampNs,
    },
    #[cfg(feature = "action-epoch-settlement")]
    AdmitPendingDeposit {
        receiver: Address,
        assets_in: u128,
        min_shares_out: u128,
        request_epoch_id: EpochId,
        now_ns: TimestampNs,
    },
    /// Governed one-time backed seed for a fresh epoch deployment. The
    /// runtime custodies the underlying assets from the governance caller and
    /// verifies the observed balance delta before dispatching this action, so
    /// the kernel only ever sees a post-transfer admission. Pristine-state,
    /// once-only, and one-for-one mint law is enforced by the dispatcher.
    #[cfg(feature = "action-epoch-settlement")]
    SeedEpochSupply {
        receiver: Address,
        assets_in: u128,
        now_ns: TimestampNs,
    },
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
            #[cfg(feature = "action-epoch-settlement")]
            Self::AdmitPendingDeposit { .. } => None,
            #[cfg(feature = "action-epoch-settlement")]
            Self::BeginEpochCutoff { .. }
            | Self::SettleEpoch { .. }
            | Self::CancelPendingWithdrawal { .. } => None,
            #[cfg(feature = "action-epoch-settlement")]
            Self::SeedEpochSupply { .. } => None,
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
            #[cfg(feature = "action-epoch-settlement")]
            Self::AdmitPendingDeposit { now_ns, .. } => Some(*now_ns),
            #[cfg(feature = "action-epoch-settlement")]
            Self::BeginEpochCutoff { now_ns, .. }
            | Self::CancelPendingWithdrawal { now_ns, .. } => Some(*now_ns),
            #[cfg(feature = "action-epoch-settlement")]
            Self::SettleEpoch { settle_now_ns, .. } => Some(*settle_now_ns),
            #[cfg(feature = "action-epoch-settlement")]
            Self::SeedEpochSupply { now_ns, .. } => Some(*now_ns),
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

/// Convert assets to shares and reject quotients above the operation's legal cap.
pub fn convert_to_shares_bounded(
    state: &VaultState,
    config: &VaultConfig,
    assets: u128,
    cap: u128,
    error: InvalidStateCode,
) -> Result<u128, KernelError> {
    conversions::convert_to_shares_bounded(state, config, assets, cap, error)
}

/// Convert shares to assets and reject quotients above the operation's legal cap.
pub fn convert_to_assets_bounded(
    state: &VaultState,
    config: &VaultConfig,
    shares: u128,
    cap: u128,
    error: InvalidStateCode,
) -> Result<u128, KernelError> {
    conversions::convert_to_assets_bounded(state, config, shares, cap, error)
}

/// Convert assets to shares with ceil rounding and reject quotients above the operation's cap.
pub fn convert_to_shares_ceil_bounded(
    state: &VaultState,
    config: &VaultConfig,
    assets: u128,
    cap: u128,
    error: InvalidStateCode,
) -> Result<u128, KernelError> {
    conversions::convert_to_shares_ceil_bounded(state, config, assets, cap, error)
}

/// Convert shares to assets with ceil rounding and reject quotients above the operation's cap.
pub fn convert_to_assets_ceil_bounded(
    state: &VaultState,
    config: &VaultConfig,
    shares: u128,
    cap: u128,
    error: InvalidStateCode,
) -> Result<u128, KernelError> {
    conversions::convert_to_assets_ceil_bounded(state, config, shares, cap, error)
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
pub(crate) fn validate_queue_head(
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
        let remaining = u128::MAX.saturating_sub(*total_supply);
        if shares > Number::from(remaining) {
            return Err(KernelError::from(
                InvalidStateCode::FeeMintOverflowTotalSupply,
            ));
        }
        let minted = shares.as_u128_trunc();
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
        #[cfg(feature = "action-epoch-settlement")]
        QueueError::InvalidRequest(_) => {
            KernelError::from(InvalidStateCode::EpochIntakeNotOpen)
        }
    }
}

#[cfg(feature = "action-immediate-deposit")]
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
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    enforce_restrictions(config, restrictions, self_id, &owner)?;
    enforce_restrictions(config, restrictions, self_id, &receiver)?;
    if !state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::DepositRequiresIdle));
    }
    if assets_in == 0 {
        return Err(KernelError::ZeroAmount);
    }

    let mut effects = Vec::new();
    #[cfg(any(feature = "action-refresh-fees", test))]
    if should_refresh_fees_before_deposit(&state, config, now_ns) {
        let mut refresh = handle_refresh_fees(state, config, now_ns)?;
        state = refresh.state;
        effects.append(&mut refresh.effects);
    }

    let shares_out = convert_to_shares_bounded(
        &state,
        config,
        assets_in,
        u128::MAX.saturating_sub(state.total_shares),
        InvalidStateCode::MintOverflowTotalShares,
    )?;
    if shares_out == 0 {
        return Err(KernelError::ZeroAmount);
    }
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
    state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, now_ns);

    effects.extend([
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
    ]);

    Ok(KernelResult::new(state, effects))
}

#[cfg(all(
    feature = "action-immediate-deposit",
    any(feature = "action-refresh-fees", test)
))]
#[inline]
fn should_refresh_fees_before_deposit(
    state: &VaultState,
    config: &VaultConfig,
    now_ns: TimestampNs,
) -> bool {
    state.total_shares > 0
        && config.fees.has_active_slot_fees()
        && now_ns > state.fee_anchor.timestamp_ns
}

#[cfg(feature = "action-atomic-exit")]
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

#[cfg(feature = "action-epoch-settlement")]
#[inline]
fn pending_withdrawal_skip_reason(
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    owner: &Address,
    receiver: &Address,
) -> Option<WithdrawalSkipReason> {
    restricted_withdraw_actor(restrictions, self_id, owner, receiver)
        .map(|_| WithdrawalSkipReason::Restricted)
}

#[cfg(not(feature = "action-epoch-settlement"))]
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
    push_refund_shares(skipped_effects,
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
            #[cfg(not(feature = "action-epoch-settlement"))]
            expected_assets: pending.expected_assets,
            reason,
        },
    });
    Ok(())
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
pub(crate) fn pending_withdrawal_head(state: &VaultState) -> Option<PendingWithdrawalHead> {
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

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
fn classify_withdrawal_head(
    head: PendingWithdrawalHead,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    now_ns: TimestampNs,
    available_assets: u128,
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
    } else if !has_actionable_withdrawal_liquidity(head.expected_assets, available_assets) {
        WithdrawalHeadOutcome::InsufficientLiquidity
    } else {
        WithdrawalHeadOutcome::Ready
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
fn has_actionable_withdrawal_liquidity(expected_assets: u128, available_assets: u128) -> bool {
    available_assets >= expected_assets
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
pub(crate) fn withdrawal_request_from_head(
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

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
fn plan_withdrawal_request(
    state: &VaultState,
    config: &VaultConfig,
    owner: Address,
    receiver: Address,
    shares: u128,
    min_assets_out: u128,
) -> Result<WithdrawalRequestPlan, KernelError> {
    let expected_assets = convert_to_assets_bounded(
        state,
        config,
        shares,
        state.total_assets,
        InvalidStateCode::RequestWithdrawExpectedAssetsExceedTotalAssets,
    )?;
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

#[cfg(feature = "action-epoch-settlement")]
pub(crate) fn pending_withdrawal_head(state: &VaultState) -> Option<PendingWithdrawalHead> {
    state
        .withdraw_queue
        .head()
        .map(|(id, pending)| PendingWithdrawalHead {
            id,
            owner: pending.owner,
            receiver: pending.receiver,
            escrow_shares: pending.escrow_shares,
            min_assets_out: pending.min_assets_out,
            requested_at_ns: pending.requested_at_ns,
            epoch_id: pending.epoch_id,
        })
}

#[cfg(feature = "action-epoch-settlement")]
#[inline]
fn settlement_floor_assets(head: &PendingWithdrawalHead, config: &VaultConfig) -> u128 {
    head.min_assets_out.max(config.min_withdrawal_assets)
}

#[cfg(feature = "action-epoch-settlement")]
#[inline]
fn classify_withdrawal_head(
    state: &VaultState,
    head: &PendingWithdrawalHead,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    now_ns: TimestampNs,
) -> WithdrawalHeadOutcome {
    if let Some(reason) =
        pending_withdrawal_skip_reason(restrictions, self_id, &head.owner, &head.receiver)
    {
        return WithdrawalHeadOutcome::Skip(reason);
    }
    if !is_past_cooldown(head.requested_at_ns, now_ns, config.withdrawal_cooldown_ns) {
        return WithdrawalHeadOutcome::CoolingDown {
            requested_at_ns: head.requested_at_ns,
        };
    }
    let Some(claim) = state
        .epoch
        .settled_claim_for(head.epoch_id, head.escrow_shares)
    else {
        return WithdrawalHeadOutcome::HeadNotSettled;
    };
    let floor = settlement_floor_assets(head, config);
    if claim == 0 || claim < floor {
        return WithdrawalHeadOutcome::BelowMinAssetsOut { min: floor, claim };
    }
    if state.idle_assets < claim {
        return WithdrawalHeadOutcome::InsufficientLiquidity;
    }
    WithdrawalHeadOutcome::Ready { claim }
}

#[cfg(feature = "action-epoch-settlement")]
#[inline]
pub(crate) fn withdrawal_request_from_head(
    state: &mut VaultState,
    head: &PendingWithdrawalHead,
    claim: u128,
) -> WithdrawalRequest {
    WithdrawalRequest {
        op_id: state.allocate_op_id(),
        request_id: head.id,
        amount: claim,
        receiver: head.receiver,
        owner: head.owner,
        escrow_shares: head.escrow_shares,
    }
}

#[cfg(feature = "action-epoch-settlement")]
/// Plan an unpriced withdrawal request.
///
/// The request stores only escrowed shares, the caller's minimum asset
/// bound, and the current open epoch. No asset claim is computed here; the
/// only pricing authority is the immutable snapshot bound when an epoch
/// settles.
#[inline]
fn plan_withdrawal_request(
    state: &VaultState,
    owner: Address,
    receiver: Address,
    shares: u128,
    min_assets_out: u128,
) -> Result<WithdrawalRequestPlan, KernelError> {
    let epoch_id = state
        .epoch
        .intake_epoch_id()
        .ok_or_else(|| KernelError::from(InvalidStateCode::EpochIntakeNotOpen))?;

    Ok(WithdrawalRequestPlan {
        owner,
        receiver,
        shares,
        min_assets_out,
        epoch_id,
    })
}

#[cfg(feature = "action-epoch-settlement")]
#[inline]
pub(crate) fn apply_withdrawal_request_plan(
    mut state: VaultState,
    config: &VaultConfig,
    self_id: &Address,
    request_plan: WithdrawalRequestPlan,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    let id = state
        .withdraw_queue
        .enqueue(
            request_plan.owner,
            request_plan.receiver,
            request_plan.shares,
            request_plan.min_assets_out,
            now_ns,
            request_plan.epoch_id,
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
                epoch_id: request_plan.epoch_id.as_u64(),
            },
        },
    ];

    Ok(KernelResult::new(state, effects))
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
pub(crate) fn apply_withdrawal_request_plan_legacy(
    mut state: VaultState,
    config: &VaultConfig,
    self_id: &Address,
    request_plan: WithdrawalRequestPlan,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
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
#[inline]
#[cfg(any(feature = "action-sync-external", test))]
fn ensure_sync_external_state_allowed(op_state: &OpState) -> Result<(), KernelError> {
    match op_state {
        OpState::Allocating(_) | OpState::Withdrawing(_) | OpState::Refreshing(_) => Ok(()),
        _ => Err(KernelError::from(
            InvalidStateCode::SyncExternalRequiresAllowedStates,
        )),
    }
}

#[inline]
#[cfg(any(feature = "action-sync-external", test))]
fn plan_external_asset_sync(
    state: &VaultState,
    new_external_assets: u128,
) -> Result<ExternalAssetSyncPlan, KernelError> {
    let new_total_assets = state
        .idle_assets
        .checked_add(new_external_assets)
        .ok_or_else(|| KernelError::from(InvalidStateCode::SyncExternalOverflowIdlePlusExternal))?;

    Ok(ExternalAssetSyncPlan {
        new_external_assets,
        new_total_assets,
    })
}

#[cfg(feature = "action-epoch-settlement")]
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

        match classify_withdrawal_head(
            state,
            &head,
            config,
            restrictions,
            self_id,
            now_ns,
        ) {
            WithdrawalHeadOutcome::Skip(reason) => {
                dequeue_skipped_withdrawal(state, self_id, skipped_effects, reason)?;
            }
            WithdrawalHeadOutcome::CoolingDown { requested_at_ns } => {
                return Ok(WithdrawalQueueOutcome::CoolingDown { requested_at_ns });
            }
            WithdrawalHeadOutcome::HeadNotSettled => {
                return Ok(WithdrawalQueueOutcome::HeadNotSettled);
            }
            WithdrawalHeadOutcome::BelowMinAssetsOut { min, claim } => {
                return Ok(WithdrawalQueueOutcome::BelowMinAssetsOut { min, claim });
            }
            WithdrawalHeadOutcome::InsufficientLiquidity => {
                return Ok(WithdrawalQueueOutcome::InsufficientLiquidity);
            }
            WithdrawalHeadOutcome::Ready { claim } => {
                return Ok(WithdrawalQueueOutcome::Ready(withdrawal_request_from_head(
                    state, &head, claim,
                )));
            }
        }
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
fn next_withdrawal_queue_outcome_legacy(
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

        match classify_withdrawal_head(
            head,
            config,
            restrictions,
            self_id,
            now_ns,
            state.idle_assets,
        ) {
            WithdrawalHeadOutcome::Skip(reason) => {
                dequeue_skipped_withdrawal(state, self_id, skipped_effects, reason)?;
            }
            WithdrawalHeadOutcome::CoolingDown { requested_at_ns } => {
                return Ok(WithdrawalQueueOutcome::CoolingDown { requested_at_ns });
            }
            WithdrawalHeadOutcome::InsufficientLiquidity => {
                return Ok(WithdrawalQueueOutcome::InsufficientLiquidity);
            }
            WithdrawalHeadOutcome::Ready => {
                return Ok(WithdrawalQueueOutcome::Ready(withdrawal_request_from_head(
                    state, head,
                )));
            }
        }
    }
}

#[cfg(feature = "action-atomic-exit")]
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

    let shares = convert_to_shares_ceil_bounded(
        &state,
        config,
        assets_out,
        state.total_shares,
        InvalidStateCode::AtomicWithdrawBurnExceedsTotalShares,
    )?;
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

#[cfg(feature = "action-atomic-exit")]
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

    let assets_out = convert_to_assets_bounded(
        &state,
        config,
        shares,
        state.idle_assets,
        InvalidStateCode::AtomicWithdrawExceedsIdleAssets,
    )?;
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

#[cfg(feature = "action-epoch-settlement")]
/// Enqueue a withdrawal request: escrow shares, bind the current open epoch.
#[allow(clippy::too_many_arguments)]
fn handle_request_withdraw(
    state: VaultState,
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

    let request_plan = plan_withdrawal_request(&state, owner, receiver, shares, min_assets_out)?;

    apply_withdrawal_request_plan(state, config, self_id, request_plan, now_ns)
}

#[cfg(not(feature = "action-epoch-settlement"))]
#[allow(clippy::too_many_arguments)]
fn handle_request_withdraw_legacy(
    state: VaultState,
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

    apply_withdrawal_request_plan_legacy(state, config, self_id, request_plan, now_ns)
}

/// Execute the next queued withdrawal after cooldown.
#[cfg(feature = "action-epoch-settlement")]
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
        WithdrawalQueueOutcome::InsufficientLiquidity => {
            if skipped_effects.is_empty() {
                Err(KernelError::from(
                    InvalidStateCode::WithdrawalLiquidityBelowMinimum,
                ))
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            }
        }
        WithdrawalQueueOutcome::HeadNotSettled => {
            if skipped_effects.is_empty() {
                Err(KernelError::from(
                    InvalidStateCode::WithdrawalEpochUnsettled,
                ))
            } else {
                Ok(KernelResult::new(state, skipped_effects))
            }
        }
        WithdrawalQueueOutcome::BelowMinAssetsOut { min, claim } => {
            if skipped_effects.is_empty() {
                Err(KernelError::Slippage { min, actual: claim })
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

#[cfg(not(feature = "action-epoch-settlement"))]
fn handle_execute_withdraw_legacy(
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
    match next_withdrawal_queue_outcome_legacy(
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
        WithdrawalQueueOutcome::InsufficientLiquidity => {
            if skipped_effects.is_empty() {
                Err(KernelError::from(
                    InvalidStateCode::WithdrawalLiquidityBelowMinimum,
                ))
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

/// Refund all escrowed shares for a pending request and repair FIFO caches.
///
/// Owner-authorized escape for unsettled or unpayable claims: cancellation
/// never repriced the request and never removed the owner from service.
#[cfg(feature = "action-epoch-settlement")]
fn handle_cancel_pending_withdrawal(
    mut state: VaultState,
    self_id: &Address,
    caller: Address,
    request_id: u64,
) -> Result<KernelResult, KernelError> {
    let pending = state
        .withdraw_queue
        .get(request_id)
        .ok_or_else(|| KernelError::from(InvalidStateCode::CancelRequestNotFound))?;
    if pending.owner != caller {
        return Err(KernelError::from(InvalidStateCode::CancelCallerNotOwner));
    }
    if matches!(state.op_state, OpState::Withdrawing(_) | OpState::Payout(_))
        && state
            .withdraw_queue
            .head()
            .is_some_and(|(head_id, _)| head_id == request_id)
    {
        return Err(KernelError::from(InvalidStateCode::CancelInFlightRequest));
    }
    let escrow_shares = pending.escrow_shares;
    let epoch_id = pending.epoch_id;
    state
        .withdraw_queue
        .remove_pending(request_id)
        .ok_or_else(|| KernelError::from(InvalidStateCode::CancelQueueRepairFailed))?;
    let mut effects = Vec::new();
    push_refund_shares(&mut effects, *self_id, caller, escrow_shares);
    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::WithdrawalCancelled {
            id: request_id,
            owner: caller,
            escrow_shares,
            epoch_id: epoch_id.as_u64(),
        },
    });
    Ok(KernelResult::new(state, effects))
}


/// Close intake for the current epoch at `cutoff_ns`.
#[cfg(feature = "action-epoch-settlement")]
fn handle_begin_epoch_cutoff(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    cutoff_ns: TimestampNs,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !state.op_state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::EpochCutoffRequiresIdle));
    }
    if is_globally_paused(config, restrictions) {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }
    if now_ns < cutoff_ns {
        return Err(KernelError::from(InvalidStateCode::EpochCutoffRejected));
    }
    let intake_epoch = state.epoch.intake_epoch;
    if has_intake_older_than_settlement(&state, intake_epoch) {
        return Err(KernelError::from(InvalidStateCode::EpochDrainRequired));
    }
    if !state.epoch.can_begin_cutoff(cutoff_ns) {
        return Err(KernelError::from(InvalidStateCode::EpochCutoffRejected));
    }
    state.epoch = state
        .epoch
        .begin_cutoff(cutoff_ns)
        .map_err(map_settlement_rejection)?;
    if !state.epoch.check_invariants() {
        return Err(KernelError::from(InvalidStateCode::EpochCutoffRejected));
    }

    Ok(KernelResult::new(
        state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::EpochCutoffStarted {
                epoch_id: intake_epoch.as_u64(),
                cutoff_ns: cutoff_ns.into(),
            },
        }],
    ))
}

/// Settle the current epoch against an accepted custodial valuation.
#[cfg(feature = "action-epoch-settlement")]
fn handle_settle_epoch(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    report: &ValuationReportRef,
    new_external_assets: u128,
    max_report_age_ns: u64,
    settle_now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !state.op_state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::EpochRequiresIdle));
    }
    if is_globally_paused(config, restrictions) {
        return Err(KernelError::Restricted(RestrictionKind::Paused));
    }
    let settling_epoch = state.epoch.intake_epoch;
    if has_intake_older_than_settlement(&state, settling_epoch) {
        return Err(KernelError::from(InvalidStateCode::EpochDrainRequired));
    }

    let settlement_nav = state
        .idle_assets
        .checked_add(new_external_assets)
        .ok_or_else(|| KernelError::from(InvalidStateCode::EpochSettlementRejected))?;
    let eligible_supply = state.total_shares;

    let snapshot = state
        .epoch
        .build_settlement_snapshot(
            report,
            settlement_nav,
            eligible_supply,
            settle_now_ns,
            max_report_age_ns,
        )
        .map_err(map_settlement_rejection)?;

    state.external_assets = new_external_assets;
    state.sync_total_assets();
    state.epoch = state
        .epoch
        .apply_settled(&snapshot)
        .map_err(map_settlement_rejection)?;
    if !state.epoch.check_invariants() || !state.check_invariant() {
        return Err(KernelError::from(InvalidStateCode::EpochSettlementRejected));
    }

    Ok(KernelResult::new(
        state,
        vec![KernelEffect::EmitEvent {
            event: KernelEvent::EpochSettled {
                epoch_id: snapshot.epoch_id().as_u64(),
                report_seq: snapshot.report_seq(),
                report_hash: *snapshot.report_hash(),
                settlement_nav: snapshot.settlement_nav(),
                eligible_supply: snapshot.eligible_supply(),
                cutoff_ns: snapshot.cutoff_ns().into(),
                as_of_ns: snapshot.as_of_ns().into(),
            },
        }],
    ))
}

/// True when the FIFO queue holds settlement-eligible intake strictly older
/// than `settling_epoch`. Such entries cannot be priced by this or any later
/// snapshot and must fully drain (repay or owner-cancel) before the epoch
/// advances, so they can never be stranded unpriceable.
#[cfg(feature = "action-epoch-settlement")]
fn has_intake_older_than_settlement(state: &VaultState, settling_epoch: EpochId) -> bool {
    state.withdraw_queue.iter().any(|(_, entry)| {
        entry.epoch_id != EpochId::MIGRATION_INTAKE
            && entry.epoch_id.is_settlement_epoch()
            && entry.epoch_id < settling_epoch
    })
}

#[cfg(feature = "action-epoch-settlement")]
fn map_settlement_rejection(_rejection: SettlementRejection) -> KernelError {
    KernelError::from(InvalidStateCode::EpochSettlementRejected)
}

/// Admit an already-custodied pending deposit.
///
/// Custody happened at request time, so no asset transfer is emitted. Shares
/// are priced only from the immutable snapshot of the settled epoch that
/// covers `request_epoch_id`, which keeps pending funds out of pricing and
/// gives every admission against one snapshot the same rate.
#[cfg(feature = "action-epoch-settlement")]
#[allow(clippy::too_many_arguments)]
fn handle_admit_pending_deposit(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    receiver: Address,
    assets_in: u128,
    min_shares_out: u128,
    request_epoch_id: EpochId,
) -> Result<KernelResult, KernelError> {
    enforce_restrictions(config, restrictions, self_id, &receiver)?;
    if !state.op_state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::DepositRequiresIdle));
    }
    if assets_in == 0 {
        return Err(KernelError::ZeroAmount);
    }
    let snapshot = state
        .epoch
        .last_settled
        .as_ref()
        .filter(|snapshot| {
            snapshot.epoch_id().is_settlement_epoch()
                && request_epoch_id.is_settlement_epoch()
                && request_epoch_id == snapshot.epoch_id()
        })
        .ok_or_else(|| KernelError::from(InvalidStateCode::DepositEpochUnsettled))?;

    let shares_out = conversions::snapshot_shares_for(
        snapshot,
        assets_in,
        u128::MAX.saturating_sub(state.total_shares),
    )?;
    if shares_out < min_shares_out {
        return Err(KernelError::Slippage {
            min: min_shares_out,
            actual: shares_out,
        });
    }
    if shares_out == 0 {
        return Err(KernelError::ZeroAmount);
    }

    // Settlement NAV is the pre-admission pricing base: it prices shares and
    // binds the request epochs it covers, and each admitted deposit correctly
    // raises accounted assets above it. Only checked arithmetic and the vault
    // accounting invariant bound the update.
    let accounting_law_holds = state.check_invariant();
    let admitted_total = state
        .total_assets
        .checked_add(assets_in)
        .ok_or_else(|| {
            KernelError::from(InvalidStateCode::DepositAdmissionOverflowTotalAssets)
        })?;
    let admitted_supply = state
        .total_shares
        .checked_add(shares_out)
        .ok_or_else(|| KernelError::from(InvalidStateCode::MintOverflowTotalShares))?;
    if !accounting_law_holds {
        return Err(KernelError::from(
            InvalidStateCode::DepositAdmissionOverflowTotalAssets,
        ));
    }

    state.idle_assets = state
        .idle_assets
        .checked_add(assets_in)
        .ok_or_else(|| KernelError::from(InvalidStateCode::DepositAdmissionOverflowIdleAssets))?;
    state.total_assets = admitted_total;
    state.total_shares = admitted_supply;
    // The bound snapshot stays immutable in storage; its epoch is copied here
    // so the emitted event cannot alias the state that is moved into the result.
    let settlement_epoch_id = match state.epoch.last_settled.as_ref() {
        Some(bound) if bound == snapshot => bound.epoch_id(),
        _ => return Err(KernelError::from(InvalidStateCode::DepositEpochUnsettled)),
    };

    Ok(KernelResult::new(
        state,
        vec![
            KernelEffect::MintShares {
                owner: receiver,
                shares: shares_out,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::DepositAdmitted {
                    receiver,
                    assets_in,
                    shares_out,
                    request_epoch_id: request_epoch_id.as_u64(),
                    settlement_epoch_id: settlement_epoch_id.as_u64(),
                },
            },
        ],
    ))
}
/// Apply a governed one-time backed seed admission (epoch-only).
///
/// The runtime custodies the underlying assets from the governance caller and
/// verifies the observed balance delta before dispatching this action, so the
/// kernel only ever sees a post-transfer admission and never emits an asset
/// transfer of its own. The law is once-only by state derivation: it applies
/// only while every accounting total is zero, the fee anchor is zero, the
/// withdrawal queue holds no entry or escrow, and epoch state is genesis with
/// no cutoff, accepted report, or settlement snapshot. Intake records, a
/// cutoff, settlement, or a prior seed each leave durable accounting or epoch
/// state that never returns to this pristine condition, so the operation can
/// never replay or run after any epoch or intake progression. Shares are
/// minted one-for-one against the runtime-verified custody amount, so a seed
/// can never create synthetic unfunded supply. Pause semantics follow
/// initialization and governance safety: the one-time bootstrap stays
/// available while paused, and the receiver remains subject to the
/// restriction lists.
#[cfg(feature = "action-epoch-settlement")]
fn handle_seed_epoch_supply(
    mut state: VaultState,
    restrictions: Option<&Restrictions>,
    receiver: Address,
    assets_in: u128,
    now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    if !state.op_state.is_idle() {
        return Err(KernelError::from(InvalidStateCode::DepositRequiresIdle));
    }
    if assets_in == 0 {
        return Err(KernelError::ZeroAmount);
    }
    if let Some(restrictions) = restrictions {
        if let Some(kind) = restrictions.is_restricted(&receiver) {
            return Err(KernelError::Restricted(kind));
        }
    }
    let pristine_seed_state = state.total_assets == 0
        && state.total_shares == 0
        && state.idle_assets == 0
        && state.external_assets == 0
        && state.fee_anchor == FeeAccrualAnchor::zero()
        && state.epoch == EpochState::genesis()
        && state.withdraw_queue.is_empty()
        && state.withdraw_queue.total_escrow_shares() == 0
        && state.withdraw_queue.check_invariants()
        && state.check_invariant();
    if !pristine_seed_state {
        return Err(KernelError::from(InvalidStateCode::EpochSeedRejected));
    }

    // One-for-one backing: the mint equals the runtime-verified custody
    // amount exactly, so seeded supply is fully funded by construction.
    let shares_out = assets_in;
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
    if !state.check_invariant() {
        return Err(KernelError::from(InvalidStateCode::EpochSeedRejected));
    }
    state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, now_ns);

    Ok(KernelResult::new(
        state,
        vec![
            KernelEffect::MintShares {
                owner: receiver,
                shares: shares_out,
            },
            KernelEffect::EmitEvent {
                event: KernelEvent::EpochSupplySeeded {
                    receiver,
                    assets_in,
                    shares_out,
                },
            },
        ],
    ))
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

/// Finish an allocation without advancing the withdrawal queue.
#[cfg(any(feature = "action-allocation-lifecycle", test))]
fn handle_finish_allocating(
    mut state: VaultState,
    _config: &VaultConfig,
    _restrictions: Option<&Restrictions>,
    _self_id: &Address,
    op_id: u64,
    _now_ns: TimestampNs,
) -> Result<KernelResult, KernelError> {
    let transition = complete_allocation(mem::take(&mut state.op_state), op_id, None);
    apply_transition_result(state, transition)
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
#[cfg(feature = "action-epoch-settlement")]
pub(crate) fn plan_payout_settlement(
    payout: &PayoutState,
    outcome: PayoutOutcome,
) -> Result<PayoutSettlement, KernelError> {
    match outcome {
        PayoutOutcome::Success => {
            if payout.burn_shares != payout.escrow_shares {
                return Err(KernelError::from(
                    InvalidStateCode::PayoutBurnMustMatchFullEscrow,
                ));
            }
            Ok(PayoutSettlement {
                burn_shares: payout.escrow_shares,
                refund_shares: 0,
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

#[cfg(feature = "action-epoch-settlement")]
pub(crate) fn apply_payout_settlement(
    state: &mut VaultState,
    payout: &PayoutState,
    settlement: PayoutSettlement,
    escrow_address: Address,
    effects: &mut Vec<KernelEffect>,
) -> Result<(), KernelError> {
    if settlement
        .burn_shares
        .checked_add(settlement.refund_shares)
        .is_none_or(|total| total != payout.escrow_shares)
    {
        return Err(KernelError::from(if settlement.success {
            InvalidStateCode::PayoutSuccessSettlementMismatch
        } else {
            InvalidStateCode::PayoutFailureSettlementMismatch
        }));
    }
    if let Some(head) = state.withdraw_queue.get(payout.request_id) {
        let claim = state
            .epoch
            .settled_claim_for(head.epoch_id, head.escrow_shares)
            .ok_or_else(|| KernelError::from(InvalidStateCode::WithdrawalEpochUnsettled))?;
        if claim != payout.amount {
            return Err(KernelError::from(InvalidStateCode::PayoutClaimMismatch));
        }
    }

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

#[cfg(not(feature = "action-epoch-settlement"))]
#[inline]
pub(crate) fn plan_payout_settlement_legacy(
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

#[cfg(not(feature = "action-epoch-settlement"))]
pub(crate) fn apply_payout_settlement_legacy(
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

    #[cfg(feature = "action-epoch-settlement")]
    let settlement = plan_payout_settlement(&payout, outcome)?;
    #[cfg(not(feature = "action-epoch-settlement"))]
    let settlement = plan_payout_settlement_legacy(&payout, outcome)?;
    #[cfg(feature = "action-epoch-settlement")]
    apply_payout_settlement(
        &mut state,
        &payout,
        settlement,
        escrow_address,
        &mut effects,
    )?;
    #[cfg(not(feature = "action-epoch-settlement"))]
    apply_payout_settlement_legacy(
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

    if total_supply > 0 && anchor.is_uninitialized() && cur_total_assets == 0 {
        state.fee_anchor = FeeAccrualAnchor::new(cur_total_assets, now_ns);
        effects.push(KernelEffect::EmitEvent {
            event: crate::effects::KernelEvent::FeesRefreshed {
                now_ns: now_ns.into(),
                total_assets: cur_total_assets,
            },
        });
        return Ok(KernelResult::new(state, effects));
    }

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
pub(crate) fn plan_emergency_reset(
    mut state: VaultState,
) -> Result<EmergencyResetOutcome, KernelError> {
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

    let mut refund_owner = None;
    let mut refund_shares = 0;

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
            refund_owner = Some(w.owner);
            refund_shares = w.escrow_shares;
            // Restore any collected assets back to idle.
            state.restore_to_idle(w.collected);
            state.withdraw_queue.dequeue();
        }
        OpState::Payout(p) => {
            refund_owner = Some(p.owner);
            refund_shares = p.escrow_shares;
            // Restore payout amount back to idle.
            state.restore_to_idle(p.amount);
            state.withdraw_queue.dequeue();
        }
    }

    state.op_state = OpState::Idle;
    state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, state.fee_anchor.timestamp_ns);

    Ok(EmergencyResetOutcome {
        state,
        op_id,
        from_code,
        refund_owner,
        refund_shares,
    })
}

#[cfg(any(feature = "action-recovery", test))]
pub(crate) fn handle_emergency_reset(
    state: VaultState,
    self_id: &Address,
) -> Result<KernelResult, KernelError> {
    let outcome = plan_emergency_reset(state)?;
    let mut effects = Vec::new();
    if let Some(owner) = outcome.refund_owner {
        push_refund_shares(&mut effects, *self_id, owner, outcome.refund_shares);
    }
    effects.push(KernelEffect::EmitEvent {
        event: KernelEvent::EmergencyResetCompleted {
            op_id: outcome.op_id,
            from_state: outcome.from_code,
        },
    });

    Ok(KernelResult::new(outcome.state, effects))
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

fn is_globally_paused(config: &VaultConfig, restrictions: Option<&Restrictions>) -> bool {
    let _ = restrictions;
    config.paused
}

#[cfg(feature = "action-epoch-settlement")]
mod planning {
    use super::*;

    pub(super) fn plan_idle_payout(
        state: &VaultState,
        min_withdrawal_assets: u128,
    ) -> Result<IdlePayoutPlan, KernelError> {
        let withdrawing = match &state.op_state {
            OpState::Withdrawing(withdrawing) => withdrawing,
            _ => {
                return Err(KernelError::from(
                    InvalidStateCode::ExecuteWithdrawRequiresIdleUseCallbacks,
                ))
            }
        };
        let request_escrow = withdrawing.escrow_shares;
        let (head_id, head) = state
            .withdraw_queue
            .head()
            .ok_or_else(|| KernelError::from(InvalidStateCode::UnexpectedEmptyQueue))?;
        if head_id != withdrawing.request_id
            || head.owner != withdrawing.owner
            || head.receiver != withdrawing.receiver
            || head.escrow_shares != request_escrow
        {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalQueueHeadMismatch,
            ));
        }
        let min_assets_out = head.min_assets_out;
        let claim = settled_claim(head, &state.epoch)
            .ok_or_else(|| KernelError::from(InvalidStateCode::WithdrawalEpochUnsettled))?;
        let floor = min_assets_out.max(min_withdrawal_assets).max(MIN_WITHDRAWAL_ASSETS);
        if claim == 0 || claim < floor {
            return Err(KernelError::Slippage {
                min: floor,
                actual: claim,
            });
        }
        let Some(settlement) = compute_idle_settlement(request_escrow, claim, state.idle_assets)
        else {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ));
        };
        if settlement.assets_out != claim || settlement.settlement.to_burn != request_escrow {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ));
        }
        Ok(IdlePayoutPlan {
            op_id: withdrawing.op_id,
            request_id: withdrawing.request_id,
            owner: withdrawing.owner,
            receiver: withdrawing.receiver,
            assets_out: settlement.assets_out,
            burn_shares: request_escrow,
        })
    }
}

#[cfg(not(feature = "action-epoch-settlement"))]
mod legacy_planning {
    use super::*;

    pub(super) fn plan_idle_payout(
        state: &VaultState,
        min_withdrawal_assets: u128,
    ) -> Result<IdlePayoutPlan, KernelError> {
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
        let _ = min_withdrawal_assets;
        if !has_actionable_withdrawal_liquidity(request_expected, available_assets) {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ));
        }

        let Some(settlement) =
            compute_idle_settlement(request_escrow, request_expected, state.idle_assets)
        else {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ));
        };

        if settlement.assets_out == 0 {
            return Err(KernelError::from(
                InvalidStateCode::WithdrawalLiquidityBelowMinimum,
            ));
        }

        Ok(IdlePayoutPlan {
            op_id: withdrawing.op_id,
            request_id: withdrawing.request_id,
            owner: withdrawing.owner,
            receiver: withdrawing.receiver,
            assets_out: settlement.assets_out,
            burn_shares: settlement.settlement.to_burn,
        })
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

    pub(super) fn convert_to_shares_bounded(
        state: &VaultState,
        config: &VaultConfig,
        assets: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        let t = effective_totals(state, config);
        mul_div_floor_bounded_u128(assets, t.supply, t.assets, cap, error)
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

    pub(super) fn convert_to_assets_bounded(
        state: &VaultState,
        config: &VaultConfig,
        shares: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        let t = effective_totals(state, config);
        mul_div_floor_bounded_u128(shares, t.assets, t.supply, cap, error)
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

    pub(super) fn convert_to_shares_ceil_bounded(
        state: &VaultState,
        config: &VaultConfig,
        assets: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        let t = effective_totals(state, config);
        mul_div_ceil_bounded_u128(assets, t.supply, t.assets, cap, error)
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

    pub(super) fn convert_to_assets_ceil_bounded(
        state: &VaultState,
        config: &VaultConfig,
        shares: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        let t = effective_totals(state, config);
        mul_div_ceil_bounded_u128(shares, t.assets, t.supply, cap, error)
    }

    fn mul_div_floor_bounded_u128(
        x: u128,
        y: u128,
        denominator: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        bounded_u128(
            mul_div_floor(Number::from(x), Number::from(y), Number::from(denominator)),
            cap,
            error,
        )
    }

    fn mul_div_ceil_bounded_u128(
        x: u128,
        y: u128,
        denominator: u128,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        bounded_u128(
            mul_div_ceil(Number::from(x), Number::from(y), Number::from(denominator)),
            cap,
            error,
        )
    }

    fn bounded_u128(
        quotient: Number,
        cap: u128,
        error: InvalidStateCode,
    ) -> Result<u128, KernelError> {
        if quotient > Number::from(cap) {
            return Err(KernelError::from(error));
        }
        Ok(quotient.as_u128_trunc())
    }

    /// Price an admission against an immutable settled snapshot:
    /// `floor(assets_in * eligible_supply / settlement_nav)`, capped at the
    /// mintable share headroom. Identical inputs always produce identical
    /// output, so every admission against one snapshot is priced alike.
    #[cfg(feature = "action-epoch-settlement")]
    pub(super) fn snapshot_shares_for(
        snapshot: &crate::state::settlement::EpochSnapshot,
        assets_in: u128,
        cap: u128,
    ) -> Result<u128, KernelError> {
        if snapshot.settlement_nav() == 0 {
            return Err(KernelError::from(
                InvalidStateCode::DepositEpochUnsettled,
            ));
        }
        mul_div_floor_bounded_u128(
            assets_in,
            snapshot.eligible_supply(),
            snapshot.settlement_nav(),
            cap,
            InvalidStateCode::MintOverflowTotalShares,
        )
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
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_action(
        mut state: VaultState,
        config: &VaultConfig,
        restrictions: Option<&Restrictions>,
        self_id: &Address,
        action: KernelAction,
    ) -> Result<KernelResult, KernelError> {
        match action {
            #[cfg(feature = "action-immediate-deposit")]
            KernelAction::Deposit {
                owner,
                receiver,
                assets_in,
                min_shares_out,
                now_ns,
            } => handle_deposit(
                state,
                config,
                restrictions,
                self_id,
                owner,
                receiver,
                assets_in,
                min_shares_out,
                now_ns,
            ),
            #[cfg(not(feature = "action-immediate-deposit"))]
            KernelAction::Deposit { .. } => Err(KernelError::NotImplemented),

            #[cfg(feature = "action-atomic-exit")]
            KernelAction::AtomicWithdraw {
                owner,
                receiver,
                operator,
                assets_out,
                max_shares_burned,
                ..
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
            #[cfg(not(feature = "action-atomic-exit"))]
            KernelAction::AtomicWithdraw { .. } => Err(KernelError::NotImplemented),

            #[cfg(feature = "action-atomic-exit")]
            KernelAction::AtomicRedeem {
                owner,
                receiver,
                operator,
                shares,
                min_assets_out,
                ..
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
            #[cfg(not(feature = "action-atomic-exit"))]
            KernelAction::AtomicRedeem { .. } => Err(KernelError::NotImplemented),

            KernelAction::RequestWithdraw {
                owner,
                receiver,
                shares,
                min_assets_out,
                now_ns,
            } => {
                #[cfg(feature = "action-epoch-settlement")]
                let dispatched = handle_request_withdraw(
                    state,
                    config,
                    restrictions,
                    self_id,
                    owner,
                    receiver,
                    shares,
                    min_assets_out,
                    now_ns,
                );
                #[cfg(not(feature = "action-epoch-settlement"))]
                let dispatched = handle_request_withdraw_legacy(
                    state,
                    config,
                    restrictions,
                    self_id,
                    owner,
                    receiver,
                    shares,
                    min_assets_out,
                    now_ns,
                );
                dispatched
            }

            KernelAction::ExecuteWithdraw { now_ns } => {
                #[cfg(feature = "action-epoch-settlement")]
                let dispatched =
                    handle_execute_withdraw(state, config, restrictions, self_id, now_ns);
                #[cfg(not(feature = "action-epoch-settlement"))]
                let dispatched =
                    handle_execute_withdraw_legacy(state, config, restrictions, self_id, now_ns);
                dispatched
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
            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::BeginEpochCutoff { cutoff_ns, now_ns } => {
                handle_begin_epoch_cutoff(state, config, restrictions, cutoff_ns, now_ns)
            }

            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::SettleEpoch {
                ref report,
                new_external_assets,
                max_report_age_ns,
                settle_now_ns,
            } => handle_settle_epoch(
                state,
                config,
                restrictions,
                report,
                new_external_assets,
                max_report_age_ns,
                settle_now_ns,
            ),

            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::AdmitPendingDeposit {
                receiver,
                assets_in,
                min_shares_out,
                request_epoch_id,
                now_ns: _,
            } => handle_admit_pending_deposit(
                state,
                config,
                restrictions,
                self_id,
                receiver,
                assets_in,
                min_shares_out,
                request_epoch_id,
            ),

            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::CancelPendingWithdrawal {
                caller,
                request_id,
                now_ns: _,
            } => handle_cancel_pending_withdrawal(state, self_id, caller, request_id),

            #[cfg(feature = "action-epoch-settlement")]
            KernelAction::SeedEpochSupply {
                receiver,
                assets_in,
                now_ns,
            } => handle_seed_epoch_supply(state, restrictions, receiver, assets_in, now_ns),

        }
    }
}

// Tests

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "action-epoch-settlement"))]
mod pending_deposit_admission_tests {
    //! Pending-deposit admission law: admission is priced only after epoch
    //! settlement, only from the immutable snapshot of the settled epoch that
    //! covers the request, and never charges custody a second time.
    use super::*;
    use crate::fee::FeesSpec;
    use crate::state::op_state::WithdrawingState;
    use crate::state::settlement::EpochSnapshot;
    use crate::state::queue::MIN_WITHDRAWAL_ASSETS;

    const ASSETS_TOTAL: u128 = 10_000;
    const SUPPLY_TOTAL: u128 = 10_000;
    const IDLE_TOTAL: u128 = 9_000;
    const EXTERNAL_TOTAL: u128 = 1_000;
    const SETTLED_EPOCH: u64 = 1;
    const CUT_NS: u64 = 1_000;
    const VALUATION_NS: u64 = 1_050;
    const NOW_NS: u64 = 1_100;

    fn address(byte: u8) -> Address {
        Address([byte; 32])
    }

    fn escrow() -> Address {
        address(0xFF)
    }

    fn receiver() -> Address {
        address(2)
    }

    fn config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            withdrawal_cooldown_ns: 0,
            max_pending_withdrawals: 16,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    fn funded_state() -> VaultState {
        VaultState::with_initial(
            ASSETS_TOTAL,
            SUPPLY_TOTAL,
            IDLE_TOTAL,
            EXTERNAL_TOTAL,
            TimestampNs::ZERO,
        )
    }

    fn cutoff(state: &mut VaultState) {
        let result = apply_action(
            state.clone(),
            &config(),
            None,
            &escrow(),
            KernelAction::BeginEpochCutoff {
                cutoff_ns: TimestampNs(CUT_NS),
                now_ns: TimestampNs(CUT_NS),
            },
        )
        .expect("cutoff accepted");
        *state = result.state;
    }

    /// Bind a settled snapshot for the epoch holding escrowed intake at the
    /// given valuation, with that intake deliberately absent from it. This is
    /// the condition admission exists to resolve.
    fn bind_settled(epoch: u64, settlement_nav: u128, eligible_supply: u128) -> VaultState {
        let mut opening = funded_state();
        cutoff(&mut opening);
        let snapshot = EpochSnapshot::bind(
            EpochId::new(epoch),
            TimestampNs(CUT_NS),
            &ValuationReportRef {
                report_seq: 1,
                as_of_ns: TimestampNs(VALUATION_NS),
                report_hash: [7u8; 32],
            },
            settlement_nav,
            eligible_supply,
        )
        .expect("snapshot law accepts the valuation");
        let epoch_state = opening
            .epoch
            .apply_settled(&snapshot)
            .expect("snapshot applies to the cutoff epoch");
        let mut settled = VaultState::with_initial(
            settlement_nav,
            eligible_supply,
            settlement_nav,
            0,
            TimestampNs(VALUATION_NS),
        );
        settled.epoch = epoch_state;
        assert_eq!(settled.epoch.last_settled.as_ref(), Some(&snapshot));
        assert!(settled.check_invariant());
        settled
    }

    /// The settled post-loss valuation of the epoch holding escrowed intake:
    /// recorded supply is unchanged while NAV has fallen below it.
    fn loss_snapshot_state() -> VaultState {
        let settled = bind_settled(SETTLED_EPOCH, IDLE_TOTAL, SUPPLY_TOTAL);
        let snapshot = settled.epoch.last_settled.as_ref().expect("snapshot bound");
        assert_eq!(snapshot.settlement_nav(), IDLE_TOTAL);
        assert_eq!(snapshot.eligible_supply(), SUPPLY_TOTAL);
        settled
    }

    fn settled_epoch(state: &VaultState) -> EpochId {
        state
            .epoch
            .last_settled
            .as_ref()
            .expect("snapshot bound")
            .epoch_id()
    }

    fn bound_snapshot(state: &VaultState) -> &EpochSnapshot {
        state
            .epoch
            .last_settled
            .as_ref()
            .expect("snapshot bound")
    }

    fn admit_with(
        state: VaultState,
        assets_in: u128,
        min_shares_out: u128,
        request_epoch_id: EpochId,
    ) -> Result<KernelResult, KernelError> {
        handle_admit_pending_deposit(
            state,
            &config(),
            None,
            &escrow(),
            receiver(),
            assets_in,
            min_shares_out,
            request_epoch_id,
        )
    }

    fn admitted_shares(result: &KernelResult) -> u128 {
        result
            .effects
            .iter()
            .find_map(|effect| match effect {
                KernelEffect::MintShares { shares, .. } => Some(*shares),
                _ => None,
            })
            .expect("admission mints shares")
    }

    fn transferred_assets(effects: &[KernelEffect]) -> bool {
        effects.iter().any(|effect| {
            matches!(
                effect,
                KernelEffect::TransferAssets { .. } | KernelEffect::TransferAssetsFrom { .. }
            )
        })
    }

    #[test]
    fn admission_after_settlement_accounts_every_custodied_asset() {
        let state = loss_snapshot_state();
        let snapshot = bound_snapshot(&state);
        let settlement_nav = snapshot.settlement_nav();
        let eligible_supply = snapshot.eligible_supply();
        let request_epoch_id = settled_epoch(&state);

        let result = admit_with(state, 1_000, 1, request_epoch_id)
            .expect("admission is accepted immediately after settlement");

        assert_eq!(
            result.state.total_assets,
            settlement_nav.checked_add(1_000).expect("summed")
        );
        assert_eq!(
            result.state.idle_assets,
            settlement_nav.checked_add(1_000).expect("summed")
        );
        assert_eq!(
            result.state.total_shares,
            eligible_supply
                .checked_add(admitted_shares(&result))
                .expect("summed")
        );
        assert!(result.state.check_invariant());
    }

    #[test]
    fn admission_mints_post_loss_shares_and_never_transfers_custodied_assets() {
        let before = loss_snapshot_state();
        let result = admit_with(before.clone(), 1_000, 1, EpochId::new(SETTLED_EPOCH))
            .expect("settled admission accepted");

        // floor(1_000 * 10_000 / 9_000) = 1_111 shares, never the 1:1 rate
        // that escrowed intake appeared to enjoy before settlement.
        assert_eq!(admitted_shares(&result), 1_111);
        assert_eq!(result.effects.len(), 2);
        assert!(result.effects.contains(&KernelEffect::MintShares {
            owner: receiver(),
            shares: 1_111,
        }));
        assert!(result.effects.contains(&KernelEffect::EmitEvent {
            event: KernelEvent::DepositAdmitted {
                receiver: receiver(),
                assets_in: 1_000,
                shares_out: 1_111,
                request_epoch_id: SETTLED_EPOCH,
                settlement_epoch_id: SETTLED_EPOCH,
            },
        }));
        assert!(!transferred_assets(&result.effects));

        let after = result.state;
        assert_eq!(after.total_assets, before.total_assets + 1_000);
        assert_eq!(after.idle_assets, before.idle_assets + 1_000);
        assert_eq!(after.total_shares, before.total_shares + 1_111);
        assert_eq!(after.epoch, before.epoch);
        assert!(after.op_state.is_idle());
        assert_eq!(
            after
                .epoch
                .last_settled
                .expect("snapshot")
                .claim_for(1_111),
            Some(999)
        );
    }

    #[test]
    fn every_admission_against_one_snapshot_prices_alike() {
        let state = loss_snapshot_state();
        let settled_epoch = settled_epoch(&state);
        let first_claim = bound_snapshot(&state).claim_for(1_111);
        let second_claim = bound_snapshot(&state).claim_for(3_333);

        let first = admit_with(state, 1_000, 1, settled_epoch).expect("first accepted");
        let first_shares = admitted_shares(&first);
        let second =
            admit_with(first.state, 1_000, 1, settled_epoch).expect("second accepted");
        assert_eq!(first_shares, admitted_shares(&second));
        assert_eq!(first_shares, 1_111);
        assert_eq!(first_claim, Some(999));
        assert_eq!(second_claim, Some(2_999));
    }

    #[test]
    fn admission_below_the_owner_minimum_leaves_state_untouched() {
        let state = loss_snapshot_state();
        let settled_epoch = settled_epoch(&state);

        assert_eq!(
            admit_with(state.clone(), 1_000, 5_000, settled_epoch),
            Err(KernelError::Slippage {
                min: 5_000,
                actual: 1_111,
            })
        );
        assert_eq!(state.total_assets, IDLE_TOTAL);
        assert_eq!(state.idle_assets, IDLE_TOTAL);
        assert_eq!(state.external_assets, 0);
        assert_eq!(state.total_shares, SUPPLY_TOTAL);
    }

    #[test]
    fn admission_rejects_output_that_prices_to_no_shares() {
        // A snapshot whose recorded eligible supply is below its own valuation
        // prices intake below one share; admission must fail closed instead of
        // crediting assets that mint nothing.
        let state = bind_settled(SETTLED_EPOCH, 10_000, 1);
        let request_epoch_id = settled_epoch(&state);

        assert_eq!(bound_snapshot(&state).claim_for(1_000), Some(10_000_000));
        assert_eq!(
            admit_with(state.clone(), 1, 0, request_epoch_id),
            Err(KernelError::ZeroAmount)
        );
        assert_eq!(
            admit_with(state, 0, 0, request_epoch_id),
            Err(KernelError::ZeroAmount)
        );
    }

    #[test]
    fn admission_requires_a_settled_epoch_covering_the_request() {
        let mut unsettled = funded_state();
        cutoff(&mut unsettled);
        let open_epoch = unsettled.epoch.intake_epoch;
        assert!(unsettled.epoch.last_settled.is_none());
        assert_eq!(
            admit_with(unsettled.clone(), 1_000, 1, open_epoch),
            Err(KernelError::InvalidState(
                InvalidStateCode::DepositEpochUnsettled
            ))
        );
        assert_eq!(unsettled.total_assets, ASSETS_TOTAL);
        assert_eq!(unsettled.total_shares, SUPPLY_TOTAL);

        let state = loss_snapshot_state();
        let later_epoch = EpochId::new(settled_epoch(&state).as_u64() + 1);
        assert_eq!(
            admit_with(state.clone(), 1_000, 1, later_epoch),
            Err(KernelError::InvalidState(
                InvalidStateCode::DepositEpochUnsettled
            ))
        );
        assert_eq!(
            admit_with(state, 1_000, 1, EpochId::MIGRATION_INTAKE),
            Err(KernelError::InvalidState(
                InvalidStateCode::DepositEpochUnsettled
            ))
        );
    }

    #[test]
    fn admission_accounts_only_within_the_bound_snapshot_valuation() {
        let state = bind_settled(SETTLED_EPOCH, 1_000, 1_000);
        let settled_epoch = settled_epoch(&state);

        let result = admit_with(state, 1, 1, settled_epoch).expect("within valuation");
        assert_eq!(admitted_shares(&result), 1);
        assert_eq!(result.state.total_assets, 1_001);
        assert_eq!(result.state.idle_assets, 1_001);
        assert_eq!(result.state.total_shares, 1_001);
    }

    #[test]
    fn admission_carries_its_deadline_and_requires_idle() {
        let action = KernelAction::AdmitPendingDeposit {
            receiver: receiver(),
            assets_in: 1_000,
            min_shares_out: 1,
            request_epoch_id: EpochId::new(SETTLED_EPOCH),
            now_ns: TimestampNs(NOW_NS),
        };
        assert_eq!(action.timestamp_ns(), Some(TimestampNs(NOW_NS)));
        assert_eq!(KernelAction::EmergencyReset.timestamp_ns(), None);

        let mut busy = loss_snapshot_state();
        let settled_epoch = settled_epoch(&busy);
        busy.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 7,
            request_id: 1,
            index: 0,
            remaining: 100,
            collected: 0,
            receiver: receiver(),
            owner: escrow(),
            escrow_shares: 100,
        });
        assert_eq!(
            admit_with(busy, 1_000, 1, settled_epoch),
            Err(KernelError::InvalidState(
                InvalidStateCode::DepositRequiresIdle
            ))
        );
    }
}


#[cfg(all(test, feature = "action-epoch-settlement"))]
mod epoch_backed_seed_tests {
    //! Governed one-time backed seed law: it applies only to pristine
    //! untouched epoch state, mints exactly one-for-one against custody the
    //! runtime observed, and is permanently excluded once accounting, intake,
    //! cutoff, or settlement state has progressed.
    use super::*;
    use crate::fee::FeesSpec;
    use crate::state::queue::MIN_WITHDRAWAL_ASSETS;
    use crate::state::settlement::EpochSnapshot;

    const SEED_ASSETS: u128 = 5_000;
    const NOW_NS: u64 = 1_100;
    const CUT_NS: u64 = 1_000;
    const VALUATION_NS: u64 = 1_050;

    fn address(byte: u8) -> Address {
        Address([byte; 32])
    }

    fn receiver() -> Address {
        address(2)
    }

    fn vault() -> Address {
        address(0xFF)
    }

    fn config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: MIN_WITHDRAWAL_ASSETS,
            withdrawal_cooldown_ns: 0,
            max_pending_withdrawals: 16,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    fn seed_with(state: VaultState, assets_in: u128) -> Result<KernelResult, KernelError> {
        apply_action(
            state,
            &config(),
            None,
            &vault(),
            KernelAction::SeedEpochSupply {
                receiver: receiver(),
                assets_in,
                now_ns: TimestampNs(NOW_NS),
            },
        )
    }

    #[test]
    fn seed_mints_exactly_matching_shares_and_never_transfers_assets() {
        let result = seed_with(VaultState::new(), SEED_ASSETS).expect("pristine seed accepted");
        assert_eq!(result.state.total_assets, SEED_ASSETS);
        assert_eq!(result.state.idle_assets, SEED_ASSETS);
        assert_eq!(result.state.total_shares, SEED_ASSETS);
        assert!(result.state.check_invariant());
        assert!(result.effects.contains(&KernelEffect::MintShares {
            owner: receiver(),
            shares: SEED_ASSETS,
        }));
        assert!(result.effects.contains(&KernelEffect::EmitEvent {
            event: KernelEvent::EpochSupplySeeded {
                receiver: receiver(),
                assets_in: SEED_ASSETS,
                shares_out: SEED_ASSETS,
            },
        }));
        assert!(
            !result.effects.iter().any(|effect| {
                matches!(
                    effect,
                    KernelEffect::TransferAssets { .. } | KernelEffect::TransferAssetsFrom { .. }
                )
            }),
            "custody precedes the kernel admission; the kernel must not transfer assets"
        );
    }

    #[test]
    fn governance_safety_keeps_the_one_time_seed_available_while_paused() {
        let result = apply_action(
            VaultState::new(),
            &VaultConfig {
                paused: true,
                ..config()
            },
            None,
            &vault(),
            KernelAction::SeedEpochSupply {
                receiver: receiver(),
                assets_in: SEED_ASSETS,
                now_ns: TimestampNs(NOW_NS),
            },
        )
        .expect("pause must not block initialization-equivalent governance bootstrap");
        assert_eq!(result.state.total_shares, SEED_ASSETS);
    }

    #[test]
    fn zero_amount_seed_is_rejected() {
        assert_eq!(
            seed_with(VaultState::new(), 0),
            Err(KernelError::ZeroAmount),
            "a zero seed would mint nothing while consuming the once-only law"
        );
    }

    #[test]
    fn seed_is_rejected_once_supply_exists() {
        let first = seed_with(VaultState::new(), SEED_ASSETS).expect("first seed accepted");
        assert_eq!(
            seed_with(first.state.clone(), SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            )),
            "a replay against funded accounting would mint unfunded shares"
        );
    }

    #[test]
    fn seed_is_rejected_when_accounting_is_nonzero() {
        let custody = VaultState::with_initial(1, 0, 1, 0, TimestampNs::ZERO);
        assert_eq!(
            seed_with(custody, SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            ))
        );
        let supply = VaultState::with_initial(1, 1, 1, 0, TimestampNs::ZERO);
        assert_eq!(
            seed_with(supply, SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            ))
        );
    }

    #[test]
    fn seed_is_rejected_after_cutoff_progression() {
        let cutoff = apply_action(
            VaultState::new(),
            &config(),
            None,
            &vault(),
            KernelAction::BeginEpochCutoff {
                cutoff_ns: TimestampNs(CUT_NS),
                now_ns: TimestampNs(NOW_NS),
            },
        )
        .expect("cutoff accepted on a fresh epoch")
        .state;
        assert_eq!(
            seed_with(cutoff, SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            )),
            "cutoff is epoch progression and must permanently exclude the seed"
        );
    }

    #[test]
    fn seed_is_rejected_after_settlement_and_after_supply_is_emptied() {
        let cutoff = apply_action(
            VaultState::new(),
            &config(),
            None,
            &vault(),
            KernelAction::BeginEpochCutoff {
                cutoff_ns: TimestampNs(CUT_NS),
                now_ns: TimestampNs(NOW_NS),
            },
        )
        .expect("cutoff accepted")
        .state;
        let snapshot = EpochSnapshot::bind(
            EpochId::FIRST_SETTLEMENT,
            TimestampNs(CUT_NS),
            &ValuationReportRef {
                report_seq: 1,
                as_of_ns: TimestampNs(VALUATION_NS),
                report_hash: [7u8; 32],
            },
            SEED_ASSETS,
            SEED_ASSETS,
        )
        .expect("snapshot law accepts the valuation");
        let settled_epoch = cutoff
            .epoch
            .apply_settled(&snapshot)
            .expect("snapshot applies to the cutoff epoch");
        let mut settled = VaultState::new();
        settled.epoch = settled_epoch.clone();
        assert_eq!(
            seed_with(settled, SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            )),
            "settled epoch state must permanently exclude the seed"
        );

        // If the entire backed supply were later redeemed to zero, the
        // settlement record remains and a seed replay can never fabricate a
        // second opening supply from nothing.
        let mut emptied = VaultState::new();
        emptied.epoch = settled_epoch;
        assert_eq!(
            seed_with(emptied, SEED_ASSETS),
            Err(KernelError::InvalidState(
                InvalidStateCode::EpochSeedRejected
            )),
            "zero accounting must not reopen seeding after an epoch has settled"
        );
    }
}