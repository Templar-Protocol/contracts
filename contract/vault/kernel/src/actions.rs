//! Kernel action dispatch for vault state transitions.
//!
//! This module defines the public `KernelAction` enum and a dispatcher that
//! applies actions to `VaultState` and returns effects.

extern crate alloc;

use core::mem;

use crate::effects::{KernelEffect, KernelEvent};
use crate::error::KernelError;
use crate::math::number::Number;
use crate::math::wad::{
    compute_fee_shares_from_assets, compute_management_fee_shares, mul_div_floor,
    total_assets_for_fee_accrual,
};
use crate::restrictions::Restrictions;
use crate::state::op_state::{OpState, TargetId};
use crate::state::queue::is_past_cooldown;
use crate::state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
use crate::transitions::{
    complete_allocation, complete_refresh, start_allocation, start_refresh, start_withdrawal,
    stop_withdrawal, WithdrawalRequest,
};
use crate::types::{Address, TimestampNs};
use alloc::vec;
use alloc::vec::Vec;
#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Result of applying a kernel action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelResult {
    pub state: VaultState,
    pub effects: Vec<KernelEffect>,
}

impl KernelResult {
    pub fn new(state: VaultState, effects: Vec<KernelEffect>) -> Self {
        Self { state, effects }
    }
}

/// Outcome for payout settlement.
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// Update the vault's paused state.
    Pause { paused: bool },
}

fn effective_totals(state: &VaultState, config: &VaultConfig) -> (u128, u128) {
    let supply = state
        .total_shares
        .saturating_add(config.virtual_shares.max(1));
    let assets = state
        .total_assets
        .saturating_add(config.virtual_assets.max(1));
    (supply, assets)
}

fn convert_to_shares(state: &VaultState, config: &VaultConfig, assets: u128) -> u128 {
    let (supply, assets_total) = effective_totals(state, config);
    u128::from(mul_div_floor(
        Number::from(assets),
        Number::from(supply),
        Number::from(assets_total),
    ))
}

fn convert_to_assets(state: &VaultState, config: &VaultConfig, shares: u128) -> u128 {
    let (supply, assets_total) = effective_totals(state, config);
    u128::from(mul_div_floor(
        Number::from(shares),
        Number::from(assets_total),
        Number::from(supply),
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

/// Apply a kernel action to state, returning updated state and effects.
pub fn apply_action(
    mut state: VaultState,
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    action: KernelAction,
) -> Result<KernelResult, KernelError> {
    if !config.is_max_pending_valid() {
        return Err(KernelError::InvalidConfig(
            "max_pending_withdrawals exceeds MAX_PENDING",
        ));
    }

    match action {
        KernelAction::Deposit {
            owner,
            receiver,
            assets_in,
            min_shares_out,
            now_ns: _,
        } => {
            enforce_restrictions(config, restrictions, self_id, &owner)?;
            enforce_restrictions(config, restrictions, self_id, &receiver)?;
            if !state.is_idle() {
                return Err(KernelError::InvalidState("deposit requires Idle"));
            }
            if assets_in == 0 {
                return Err(KernelError::Slippage {
                    min: min_shares_out,
                    actual: 0,
                });
            }

            let shares_out = convert_to_shares(&state, config, assets_in);
            if shares_out < min_shares_out {
                return Err(KernelError::Slippage {
                    min: min_shares_out,
                    actual: shares_out,
                });
            }

            state.total_assets = state.total_assets.saturating_add(assets_in);
            state.idle_assets = state.idle_assets.saturating_add(assets_in);
            state.total_shares = state.total_shares.saturating_add(shares_out);

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
        KernelAction::RequestWithdraw {
            owner,
            receiver,
            shares,
            min_assets_out,
            now_ns,
        } => {
            enforce_restrictions(config, restrictions, self_id, &owner)?;
            enforce_restrictions(config, restrictions, self_id, &receiver)?;
            if !state.is_idle() {
                return Err(KernelError::InvalidState("request_withdraw requires Idle"));
            }
            if shares == 0 {
                return Err(KernelError::Slippage {
                    min: min_assets_out,
                    actual: 0,
                });
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
                .map_err(|_| KernelError::QueueFull)?;

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
        KernelAction::ExecuteWithdraw { now_ns } => {
            if state.op_state.is_withdrawing() {
                return Err(KernelError::InvalidState(
                    "execute_withdraw requires Idle (use withdrawal callbacks to advance)",
                ));
            }
            if !state.op_state.is_idle() {
                return Err(KernelError::InvalidState(
                    "execute_withdraw requires Idle or Withdrawing",
                ));
            }

            let Some((_, pending_ref)) = state.withdraw_queue.head() else {
                return Err(KernelError::EmptyQueue);
            };
            let pending = pending_ref.clone();

            enforce_restrictions(config, restrictions, self_id, &pending.owner)?;
            enforce_restrictions(config, restrictions, self_id, &pending.receiver)?;

            if !is_past_cooldown(
                pending.requested_at_ns,
                now_ns,
                config.withdrawal_cooldown_ns,
            ) {
                return Err(KernelError::Cooldown {
                    requested_at: pending.requested_at_ns,
                    now: now_ns,
                    cooldown_ns: config.withdrawal_cooldown_ns,
                });
            }

            let op_id = state.allocate_op_id();
            let request = WithdrawalRequest {
                op_id,
                amount: pending.expected_assets,
                receiver: pending.receiver,
                owner: pending.owner,
                escrow_shares: pending.escrow_shares,
            };

            let result = start_withdrawal(mem::take(&mut state.op_state), request)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;

            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::BeginAllocating { op_id, plan, .. } => {
            let result = start_allocation(mem::take(&mut state.op_state), plan, op_id)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::FinishAllocating { op_id, now_ns } => {
            let pending = state.withdraw_queue.head().and_then(|(_, w)| {
                if is_past_cooldown(
                    w.requested_at_ns,
                    now_ns,
                    config.withdrawal_cooldown_ns,
                ) {
                    Some(w.clone())
                } else {
                    None
                }
            });

            let pending_req = pending.map(|w| WithdrawalRequest {
                op_id: state.allocate_op_id(),
                amount: w.expected_assets,
                receiver: w.receiver,
                owner: w.owner,
                escrow_shares: w.escrow_shares,
            });

            let result = complete_allocation(mem::take(&mut state.op_state), op_id, pending_req)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::BeginRefreshing { op_id, plan, .. } => {
            let result = start_refresh(mem::take(&mut state.op_state), plan, op_id)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::FinishRefreshing { op_id, .. } => {
            let result =
                complete_refresh(mem::take(&mut state.op_state), op_id).map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::SyncExternalAssets {
            new_external_assets,
            op_id,
            ..
        } => {
            let Some(active_op_id) = state.op_state.op_id() else {
                return Err(KernelError::InvalidState(
                    "sync_external_assets requires active op",
                ));
            };

            if active_op_id != op_id {
                return Err(KernelError::OpIdMismatch {
                    expected: active_op_id,
                    actual: op_id,
                });
            }

            match state.op_state {
                OpState::Allocating(_) | OpState::Withdrawing(_) | OpState::Refreshing(_) => {}
                _ => {
                    return Err(KernelError::InvalidState(
                        "sync_external_assets requires Allocating/Withdrawing/Refreshing",
                    ));
                }
            }

            // Overflow protection: idle_assets + new_external must fit in u128.
            let new_total = state
                .idle_assets
                .checked_add(new_external_assets)
                .ok_or(KernelError::InvalidState(
                    "sync_external_assets overflow: idle + external exceeds u128",
                ))?;

            // Sanity bound: prevent a compromised allocator from inflating
            // total_assets beyond 2x the previous value. Legitimate market
            // gains within a single operation window are bounded well below
            // this. Manual reconciliation should be used for larger swings.
            if state.total_assets > 0 && new_total > state.total_assets.saturating_mul(2) {
                return Err(KernelError::InvalidState(
                    "sync_external_assets would more than double total_assets",
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
        KernelAction::AbortRefreshing { op_id } => {
            let current_op_id = state.op_state.op_id().ok_or(KernelError::InvalidState(
                "abort_refreshing requires active op",
            ))?;
            if current_op_id != op_id {
                return Err(KernelError::OpIdMismatch {
                    expected: current_op_id,
                    actual: op_id,
                });
            }

            if !matches!(state.op_state, OpState::Refreshing(_)) {
                return Err(KernelError::InvalidState(
                    "abort_refreshing requires Refreshing",
                ));
            }

            state.op_state = OpState::Idle;
            Ok(KernelResult::new(state, vec![]))
        }
        KernelAction::AbortAllocating {
            op_id,
            restore_idle,
        } => {
            let alloc = match &state.op_state {
                OpState::Allocating(s) => s,
                _ => {
                    return Err(KernelError::InvalidState(
                        "abort_allocating requires Allocating",
                    ))
                }
            };

            if alloc.op_id != op_id {
                return Err(KernelError::OpIdMismatch {
                    expected: alloc.op_id,
                    actual: op_id,
                });
            }
            if restore_idle != alloc.remaining {
                return Err(KernelError::InvalidState(
                    "abort_allocating restore_idle mismatch",
                ));
            }

            state.idle_assets = state.idle_assets.saturating_add(restore_idle);
            state.total_assets = state.idle_assets.saturating_add(state.external_assets);
            state.op_state = OpState::Idle;
            Ok(KernelResult::new(state, vec![]))
        }
        KernelAction::AbortWithdrawing {
            op_id,
            refund_shares,
        } => {
            let withdraw = match &state.op_state {
                OpState::Withdrawing(s) => s,
                _ => {
                    return Err(KernelError::InvalidState(
                        "abort_withdrawing requires Withdrawing",
                    ))
                }
            };

            if withdraw.op_id != op_id {
                return Err(KernelError::OpIdMismatch {
                    expected: withdraw.op_id,
                    actual: op_id,
                });
            }
            if refund_shares != withdraw.escrow_shares {
                return Err(KernelError::InvalidState(
                    "abort_withdrawing refund_shares mismatch",
                ));
            }

            let Some((_, pending)) = state.withdraw_queue.head() else {
                return Err(KernelError::EmptyQueue);
            };
            if pending.owner != withdraw.owner
                || pending.receiver != withdraw.receiver
                || pending.escrow_shares != withdraw.escrow_shares
            {
                return Err(KernelError::InvalidState("withdrawal queue head mismatch"));
            }

            let result = stop_withdrawal(mem::take(&mut state.op_state), op_id, *self_id)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            state.withdraw_queue.dequeue();
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::SettlePayout { op_id, outcome } => {
            let payout = match mem::take(&mut state.op_state) {
                OpState::Payout(s) => s,
                _ => return Err(KernelError::InvalidState("settle_payout requires Payout")),
            };

            if payout.op_id != op_id {
                return Err(KernelError::OpIdMismatch {
                    expected: payout.op_id,
                    actual: op_id,
                });
            }

            let Some((_, pending)) = state.withdraw_queue.head() else {
                return Err(KernelError::EmptyQueue);
            };
            if pending.owner != payout.owner
                || pending.receiver != payout.receiver
                || pending.escrow_shares != payout.escrow_shares
            {
                return Err(KernelError::InvalidState("withdrawal queue head mismatch"));
            }

            let escrow_address = *self_id;
            let mut effects = Vec::new();

            let (burn_shares, refund_shares, amount, success) = match outcome {
                PayoutOutcome::Success {
                    burn_shares: burn_amount,
                    refund_shares: refund_amount,
                } => {
                    if burn_amount.saturating_add(refund_amount) != payout.escrow_shares {
                        return Err(KernelError::InvalidState(
                            "payout success settlement mismatch",
                        ));
                    }

                    if burn_amount > 0 {
                        effects.push(KernelEffect::BurnShares {
                            owner: escrow_address,
                            shares: burn_amount,
                        });
                        state.total_shares = state.total_shares.saturating_sub(burn_amount);
                    }
                    if refund_amount > 0 {
                        effects.push(KernelEffect::TransferShares {
                            from: escrow_address,
                            to: payout.owner,
                            shares: refund_amount,
                        });
                    }

                    state.op_state = OpState::Idle;
                    (burn_amount, refund_amount, payout.amount, true)
                }
                PayoutOutcome::Failure {
                    restore_idle,
                    refund_shares: refund_amount,
                } => {
                    if refund_amount != payout.escrow_shares {
                        return Err(KernelError::InvalidState(
                            "payout failure settlement mismatch",
                        ));
                    }
                    if restore_idle != payout.amount {
                        return Err(KernelError::InvalidState(
                            "payout failure restore_idle must equal payout.amount",
                        ));
                    }

                    if refund_amount > 0 {
                        effects.push(KernelEffect::TransferShares {
                            from: escrow_address,
                            to: payout.owner,
                            shares: refund_amount,
                        });
                    }

                    state.idle_assets = state.idle_assets.saturating_add(restore_idle);
                    state.total_assets = state.idle_assets.saturating_add(state.external_assets);
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
        KernelAction::Pause { paused } => Ok(KernelResult::new(
            state,
            vec![KernelEffect::EmitEvent {
                event: crate::effects::KernelEvent::PauseUpdated { paused },
            }],
        )),
        KernelAction::RefreshFees { now_ns } => {
            // Reject backwards time to prevent fee calculation issues
            if now_ns < state.fee_anchor.timestamp_ns {
                return Err(KernelError::InvalidState("fee refresh timestamp cannot go backwards"));
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
            if mgmt_shares > Number::zero() {
                let minted: u128 = mgmt_shares.into();
                effects.push(KernelEffect::MintShares {
                    owner: config.fees.management.recipient,
                    shares: minted,
                });
                total_supply = total_supply.saturating_add(minted);
            }

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
            if perf_shares > Number::zero() {
                let minted: u128 = perf_shares.into();
                effects.push(KernelEffect::MintShares {
                    owner: config.fees.performance.recipient,
                    shares: minted,
                });
                total_supply = total_supply.saturating_add(minted);
            }

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
    }
}

fn enforce_restrictions(
    config: &VaultConfig,
    restrictions: Option<&Restrictions>,
    self_id: &Address,
    actor: &Address,
) -> Result<(), KernelError> {
    if config.paused {
        return Err(KernelError::Restricted(Restrictions::Paused));
    }
    if let Some(restrictions) = restrictions {
        if let Some(reason) = restrictions.is_restricted(actor, self_id) {
            return Err(KernelError::Restricted(reason));
        }
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::KernelEvent;
    use crate::fee::{FeeSlot, FeesSpec};
    use crate::math::wad::Wad;
    use crate::state::op_state::WithdrawingState;
    use crate::state::queue::{DEFAULT_COOLDOWN_NS, MAX_PENDING};

    fn addr(tag: u8) -> Address {
        [tag; 32]
    }

    fn test_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 0,
            withdrawal_cooldown_ns: DEFAULT_COOLDOWN_NS,
            max_pending_withdrawals: 10,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
    }

    #[test]
    fn invalid_max_pending_rejected() {
        let state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let mut config = test_config();
        config.max_pending_withdrawals = (MAX_PENDING as u32).saturating_add(1);

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Pause { paused: false },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidConfig(
                "max_pending_withdrawals exceeds MAX_PENDING"
            ))
        ));
    }

    #[test]
    fn request_withdraw_enqueues_and_emits_event() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(1),
                receiver: addr(2),
                shares: 100,
                min_assets_out: 0,
                now_ns: 0,
            },
        )
        .unwrap();

        assert_eq!(result.state.withdraw_queue.len(), 1);
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::TransferShares { .. })
        ));
        assert!(matches!(
            result.effects.get(1),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::WithdrawalRequested { .. }
            })
        ));
    }

    #[test]
    fn execute_withdraw_idle_starts_withdrawal() {
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();
        let owner = addr(3);
        let receiver = addr(4);

        let _ = state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw {
                now_ns: DEFAULT_COOLDOWN_NS + 1,
            },
        )
        .unwrap();

        let withdraw = result.state.op_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, 0);
        assert_eq!(withdraw.owner, owner);
        assert_eq!(withdraw.receiver, receiver);
        assert_eq!(withdraw.escrow_shares, 100);
        assert_eq!(withdraw.remaining, 100);
    }

    #[test]
    fn execute_withdraw_withdrawing_advances_index() {
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();
        let owner = addr(5);
        let receiver = addr(6);

        let _ = state
            .withdraw_queue
            .enqueue(owner, receiver, 200, 200, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 7,
            index: 0,
            remaining: 200,
            collected: 0,
            receiver,
            owner,
            escrow_shares: 200,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw { now_ns: 0 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
            ))
        ));
    }

    #[test]
    fn deposit_blocked_when_paused() {
        let state = VaultState::new();
        let mut config = test_config();
        config.paused = true;

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Deposit {
                owner: addr(1),
                receiver: addr(2),
                assets_in: 10,
                min_shares_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::Restricted(Restrictions::Paused))
        ));
    }

    #[test]
    fn request_withdraw_blocked_by_blacklist() {
        use alloc::collections::BTreeSet;

        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();
        let mut blacklist = BTreeSet::new();
        blacklist.insert(addr(9));
        let restrictions = Restrictions::BlackList(blacklist);

        let result = apply_action(
            state,
            &config,
            Some(&restrictions),
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(9),
                receiver: addr(3),
                shares: 10,
                min_assets_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::Restricted(Restrictions::BlackList(_)))
        ));
    }

    // =========================================================================
    // Deposit action tests
    // =========================================================================

    #[test]
    fn deposit_success() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Deposit {
                owner: addr(1),
                receiver: addr(2),
                assets_in: 500,
                min_shares_out: 0,
                now_ns: 0,
            },
        )
        .unwrap();

        // With virtual_assets/shares = 0, ratio is 1:1 after adjustments
        assert_eq!(result.state.total_assets, 1_500);
        assert_eq!(result.state.idle_assets, 1_500);
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::TransferAssetsFrom { .. })
        ));
        assert!(matches!(
            result.effects.get(1),
            Some(KernelEffect::MintShares { .. })
        ));
        assert!(matches!(
            result.effects.get(2),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::DepositProcessed { .. }
            })
        ));
    }

    #[test]
    fn deposit_emits_transfer_assets_from_owner() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();
        let self_id = addr(0xAB);
        let owner = addr(1);

        let result = apply_action(
            state,
            &config,
            None,
            &self_id,
            KernelAction::Deposit {
                owner,
                receiver: addr(2),
                assets_in: 250,
                min_shares_out: 0,
                now_ns: 0,
            },
        )
        .unwrap();

        let transfer = result.effects.iter().find_map(|effect| match effect {
            KernelEffect::TransferAssetsFrom { from, to, amount } => Some((*from, *to, *amount)),
            _ => None,
        });

        assert_eq!(transfer, Some((owner, self_id, 250)));
    }

    #[test]
    fn deposit_zero_assets_fails_slippage() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Deposit {
                owner: addr(1),
                receiver: addr(2),
                assets_in: 0,
                min_shares_out: 1,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::Slippage { min: 1, actual: 0 })
        ));
    }

    #[test]
    fn deposit_slippage_check_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Deposit {
                owner: addr(1),
                receiver: addr(2),
                assets_in: 100,
                min_shares_out: 1_000_000, // Way more than we can get
                now_ns: 0,
            },
        );

        assert!(matches!(result, Err(KernelError::Slippage { .. })));
    }

    #[test]
    fn deposit_not_idle_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 500,
            plan: vec![(0, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Deposit {
                owner: addr(1),
                receiver: addr(2),
                assets_in: 100,
                min_shares_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState("deposit requires Idle"))
        ));
    }

    // =========================================================================
    // RequestWithdraw action tests
    // =========================================================================

    #[test]
    fn request_withdraw_zero_shares_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(1),
                receiver: addr(2),
                shares: 0,
                min_assets_out: 1,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::Slippage { min: 1, actual: 0 })
        ));
    }

    #[test]
    fn request_withdraw_slippage_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(1),
                receiver: addr(2),
                shares: 10,
                min_assets_out: 1_000_000, // Way more than we can get
                now_ns: 0,
            },
        );

        assert!(matches!(result, Err(KernelError::Slippage { .. })));
    }

    #[test]
    fn request_withdraw_min_withdrawal_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let mut config = test_config();
        config.min_withdrawal_assets = 1_000;

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(1),
                receiver: addr(2),
                shares: 10,
                min_assets_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(result, Err(KernelError::MinWithdrawal { .. })));
    }

    #[test]
    fn request_withdraw_not_idle_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 500,
            plan: vec![(0, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(1),
                receiver: addr(2),
                shares: 100,
                min_assets_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState("request_withdraw requires Idle"))
        ));
    }

    #[test]
    fn request_withdraw_queue_full_fails() {
        let mut state = VaultState::with_initial(10_000, 10_000, 10_000, 0, 0);
        let mut config = test_config();
        config.max_pending_withdrawals = 2;

        // Fill the queue
        state
            .withdraw_queue
            .enqueue(
                addr(1),
                addr(1),
                100,
                100,
                0,
                config.max_pending_withdrawals,
            )
            .unwrap();
        state
            .withdraw_queue
            .enqueue(
                addr(2),
                addr(2),
                100,
                100,
                0,
                config.max_pending_withdrawals,
            )
            .unwrap();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RequestWithdraw {
                owner: addr(3),
                receiver: addr(3),
                shares: 100,
                min_assets_out: 0,
                now_ns: 0,
            },
        );

        assert!(matches!(result, Err(KernelError::QueueFull)));
    }

    // =========================================================================
    // ExecuteWithdraw action tests
    // =========================================================================

    #[test]
    fn execute_withdraw_empty_queue_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw {
                now_ns: DEFAULT_COOLDOWN_NS + 1,
            },
        );

        assert!(matches!(result, Err(KernelError::EmptyQueue)));
    }

    #[test]
    fn execute_withdraw_cooldown_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        state
            .withdraw_queue
            .enqueue(
                addr(1),
                addr(2),
                100,
                100,
                1_000_000,
                config.max_pending_withdrawals,
            )
            .unwrap();

        // Not enough time passed
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw { now_ns: 1_000_000 },
        );

        assert!(matches!(result, Err(KernelError::Cooldown { .. })));
    }

    #[test]
    fn execute_withdraw_wrong_state_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 500,
            plan: vec![(0, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw { now_ns: 0 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "execute_withdraw requires Idle or Withdrawing"
            ))
        ));
    }

    #[test]
    fn execute_withdraw_queue_head_mismatch_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();
        let owner = addr(5);
        let receiver = addr(6);

        // Queue has different owner than op_state
        state
            .withdraw_queue
            .enqueue(
                addr(99),
                addr(99),
                200,
                200,
                0,
                config.max_pending_withdrawals,
            )
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 7,
            index: 0,
            remaining: 200,
            collected: 0,
            receiver,
            owner,
            escrow_shares: 200,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw { now_ns: 0 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
            ))
        ));
    }

    // =========================================================================
    // BeginAllocating action tests
    // =========================================================================

    #[test]
    fn begin_allocating_success() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::BeginAllocating {
                op_id: 1,
                plan: vec![(1, 500)],
                now_ns: 0,
            },
        )
        .unwrap();

        assert!(result.state.op_state.as_allocating().is_some());
    }

    // =========================================================================
    // FinishAllocating action tests
    // =========================================================================

    #[test]
    fn finish_allocating_success() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 1,
            remaining: 0,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::FinishAllocating {
                op_id: 1,
                now_ns: 0,
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
    }

    #[test]
    fn finish_allocating_with_pending_withdrawal() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let owner = addr(10);
        let receiver = addr(11);
        let config = test_config();

        // Add a pending withdrawal that's past cooldown
        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 5,
            index: 1,
            remaining: 0,
            plan: vec![(1, 500)],
        });

        // now_ns is past cooldown (DEFAULT_COOLDOWN_NS + request time of 0)
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::FinishAllocating {
                op_id: 5,
                now_ns: DEFAULT_COOLDOWN_NS + 1,
            },
        )
        .unwrap();

        // Should transition to Withdrawing instead of Idle
        assert!(result.state.op_state.as_withdrawing().is_some());
    }

    #[test]
    fn finish_allocating_with_pending_withdrawal_not_past_cooldown() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let owner = addr(10);
        let receiver = addr(11);
        let config = test_config();

        // Add a pending withdrawal that's NOT past cooldown
        state
            .withdraw_queue
            .enqueue(
                owner,
                receiver,
                100,
                100,
                DEFAULT_COOLDOWN_NS,
                config.max_pending_withdrawals,
            )
            .unwrap();

        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 6,
            index: 1,
            remaining: 0,
            plan: vec![(1, 500)],
        });

        // now_ns is not past cooldown
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::FinishAllocating {
                op_id: 6,
                now_ns: DEFAULT_COOLDOWN_NS,
            },
        )
        .unwrap();

        // Should transition to Idle since withdrawal is not ready
        assert!(result.state.is_idle());
    }

    #[test]
    fn execute_withdraw_withdrawing_empty_queue() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();

        // State is Withdrawing but queue is empty (shouldn't happen in practice)
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 8,
            index: 0,
            remaining: 100,
            collected: 0,
            owner: addr(1),
            receiver: addr(2),
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::ExecuteWithdraw { now_ns: 0 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "execute_withdraw requires Idle (use withdrawal callbacks to advance)"
            ))
        ));
    }

    // =========================================================================
    // BeginRefreshing action tests
    // =========================================================================

    #[test]
    fn begin_refreshing_success() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::BeginRefreshing {
                op_id: 1,
                plan: vec![1],
                now_ns: 0,
            },
        )
        .unwrap();

        assert!(result.state.op_state.as_refreshing().is_some());
    }

    // =========================================================================
    // FinishRefreshing action tests
    // =========================================================================

    #[test]
    fn finish_refreshing_success() {
        use crate::state::op_state::RefreshingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id: 2,
            index: 1,
            plan: vec![1],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::FinishRefreshing {
                op_id: 2,
                now_ns: 0,
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
    }

    // =========================================================================
    // SyncExternalAssets action tests
    // =========================================================================

    #[test]
    fn sync_external_assets_allocating() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 3,
            index: 0,
            remaining: 500,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 700,
                op_id: 3,
                now_ns: 0,
            },
        )
        .unwrap();

        assert_eq!(result.state.external_assets, 700);
        assert_eq!(result.state.total_assets, 1_200); // idle(500) + external(700)
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::ExternalAssetsSynced { .. }
            })
        ));
    }

    #[test]
    fn sync_external_assets_withdrawing() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 4,
            index: 0,
            remaining: 100,
            collected: 0,
            owner: addr(1),
            receiver: addr(2),
            escrow_shares: 100,
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 400,
                op_id: 4,
                now_ns: 0,
            },
        )
        .unwrap();

        assert_eq!(result.state.external_assets, 400);
        assert_eq!(result.state.total_assets, 900);
    }

    #[test]
    fn sync_external_assets_refreshing() {
        use crate::state::op_state::RefreshingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id: 5,
            index: 0,
            plan: vec![1],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 600,
                op_id: 5,
                now_ns: 0,
            },
        )
        .unwrap();

        assert_eq!(result.state.external_assets, 600);
    }

    #[test]
    fn sync_external_assets_idle_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 500,
                op_id: 1,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "sync_external_assets requires active op"
            ))
        ));
    }

    #[test]
    fn sync_external_assets_op_id_mismatch_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 500,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 500,
                op_id: 99, // Wrong op_id
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::OpIdMismatch {
                expected: 10,
                actual: 99
            })
        ));
    }

    #[test]
    fn sync_external_assets_payout_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Payout(PayoutState {
            op_id: 6,
            owner: addr(1),
            receiver: addr(2),
            amount: 50,
            escrow_shares: 100,
            burn_shares: 50,
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 500,
                op_id: 6,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "sync_external_assets requires Allocating/Withdrawing/Refreshing"
            ))
        ));
    }

    #[test]
    fn sync_external_assets_rejects_doubling() {
        use crate::state::op_state::AllocatingState;
        // total_assets = 1000; trying to set external to 2001 would make new total > 2x
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 500,
            plan: vec![(0, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 2_001,
                op_id: 1,
                now_ns: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "sync_external_assets would more than double total_assets"
            ))
        ));
    }

    #[test]
    fn sync_external_assets_allows_up_to_double() {
        use crate::state::op_state::AllocatingState;
        // total_assets = 1000; setting external to 1000 with idle=1000 => new total=2000 = 2x, OK
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 1,
            index: 0,
            remaining: 500,
            plan: vec![(0, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SyncExternalAssets {
                new_external_assets: 1_000,
                op_id: 1,
                now_ns: 0,
            },
        );

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.state.total_assets, 2_000);
    }

    // =========================================================================
    // AbortRefreshing action tests
    // =========================================================================

    #[test]
    fn abort_refreshing_success() {
        use crate::state::op_state::RefreshingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id: 7,
            index: 0,
            plan: vec![1],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortRefreshing { op_id: 7 },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert!(result.effects.is_empty());
    }

    #[test]
    fn abort_refreshing_wrong_state_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortRefreshing { op_id: 1 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_refreshing requires active op"
            ))
        ));
    }

    #[test]
    fn abort_refreshing_op_id_mismatch_fails() {
        use crate::state::op_state::RefreshingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Refreshing(RefreshingState {
            op_id: 10,
            index: 0,
            plan: vec![1],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortRefreshing { op_id: 99 },
        );

        assert!(matches!(
            result,
            Err(KernelError::OpIdMismatch {
                expected: 10,
                actual: 99
            })
        ));
    }

    #[test]
    fn abort_refreshing_wrong_op_type_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 500,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortRefreshing { op_id: 10 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_refreshing requires Refreshing"
            ))
        ));
    }

    // =========================================================================
    // AbortAllocating action tests
    // =========================================================================

    #[test]
    fn abort_allocating_success() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(800, 1_000, 300, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 8,
            index: 0,
            remaining: 200,
            plan: vec![(1, 200)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortAllocating {
                op_id: 8,
                restore_idle: 200,
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert_eq!(result.state.idle_assets, 500); // 300 + 200 restored
        assert_eq!(result.state.total_assets, 1000); // 500 idle + 500 external
    }

    #[test]
    fn abort_allocating_wrong_state_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortAllocating {
                op_id: 1,
                restore_idle: 0,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_allocating requires Allocating"
            ))
        ));
    }

    #[test]
    fn abort_allocating_op_id_mismatch_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 500,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortAllocating {
                op_id: 99,
                restore_idle: 500,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::OpIdMismatch {
                expected: 10,
                actual: 99
            })
        ));
    }

    #[test]
    fn abort_allocating_restore_mismatch_fails() {
        use crate::state::op_state::AllocatingState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        state.op_state = OpState::Allocating(AllocatingState {
            op_id: 10,
            index: 0,
            remaining: 500,
            plan: vec![(1, 500)],
        });
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortAllocating {
                op_id: 10,
                restore_idle: 999, // Wrong amount
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_allocating restore_idle mismatch"
            ))
        ));
    }

    // =========================================================================
    // AbortWithdrawing action tests
    // =========================================================================

    #[test]
    fn abort_withdrawing_success() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 9,
            index: 0,
            remaining: 100,
            collected: 0,
            owner,
            receiver,
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 9,
                refund_shares: 100,
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert_eq!(result.state.withdraw_queue.len(), 0);
    }

    #[test]
    fn abort_withdrawing_wrong_state_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 1,
                refund_shares: 100,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_withdrawing requires Withdrawing"
            ))
        ));
    }

    #[test]
    fn abort_withdrawing_op_id_mismatch_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            collected: 0,
            owner,
            receiver,
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 99,
                refund_shares: 100,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::OpIdMismatch {
                expected: 10,
                actual: 99
            })
        ));
    }

    #[test]
    fn abort_withdrawing_refund_mismatch_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            collected: 0,
            owner,
            receiver,
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 10,
                refund_shares: 999,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "abort_withdrawing refund_shares mismatch"
            ))
        ));
    }

    #[test]
    fn abort_withdrawing_queue_head_mismatch_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();

        // Queue has different user
        state
            .withdraw_queue
            .enqueue(
                addr(99),
                addr(99),
                100,
                100,
                0,
                config.max_pending_withdrawals,
            )
            .unwrap();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            collected: 0,
            owner: addr(1),
            receiver: addr(2),
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 10,
                refund_shares: 100,
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState("withdrawal queue head mismatch"))
        ));
    }

    #[test]
    fn abort_withdrawing_empty_queue_fails() {
        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();

        state.op_state = OpState::Withdrawing(WithdrawingState {
            op_id: 10,
            index: 0,
            remaining: 100,
            collected: 0,
            owner: addr(1),
            receiver: addr(2),
            escrow_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::AbortWithdrawing {
                op_id: 10,
                refund_shares: 100,
            },
        );

        assert!(matches!(result, Err(KernelError::EmptyQueue)));
    }

    // =========================================================================
    // SettlePayout action tests
    // =========================================================================

    #[test]
    fn settle_payout_success_burn_only() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 11,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 11,
                outcome: PayoutOutcome::Success {
                    burn_shares: 100,
                    refund_shares: 0,
                },
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert_eq!(result.state.total_shares, 900); // 1000 - 100 burned
        assert_eq!(result.state.withdraw_queue.len(), 0);
        let (burn_owner, burn_shares) = result
            .effects
            .iter()
            .find_map(|e| match e {
                KernelEffect::BurnShares { owner, shares } => Some((*owner, *shares)),
                _ => None,
            })
            .expect("missing BurnShares effect");
        assert_eq!(burn_owner, addr(0xFF));
        assert_eq!(burn_shares, 100);
        let event = result
            .effects
            .iter()
            .find_map(|e| match e {
                KernelEffect::EmitEvent {
                    event:
                        KernelEvent::PayoutCompleted {
                            op_id,
                            success,
                            burn_shares,
                            refund_shares,
                            amount,
                        },
                } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
                _ => None,
            })
            .expect("missing PayoutCompleted event");
        assert_eq!(event, (11, true, 100, 0, 100));
    }

    #[test]
    fn settle_payout_success_partial_refund() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 12,
            owner,
            receiver,
            amount: 50,
            escrow_shares: 100,
            burn_shares: 50,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 12,
                outcome: PayoutOutcome::Success {
                    burn_shares: 50,
                    refund_shares: 50,
                },
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert_eq!(result.state.total_shares, 950);
        assert_eq!(result.effects.len(), 3); // BurnShares + TransferShares + PayoutCompleted
        let event = result
            .effects
            .iter()
            .find_map(|e| match e {
                KernelEffect::EmitEvent {
                    event:
                        KernelEvent::PayoutCompleted {
                            op_id,
                            success,
                            burn_shares,
                            refund_shares,
                            amount,
                        },
                } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
                _ => None,
            })
            .expect("missing PayoutCompleted event");
        assert_eq!(event, (12, true, 50, 50, 50));
    }

    #[test]
    fn settle_payout_failure() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(900, 1_000, 400, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 13,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 13,
                outcome: PayoutOutcome::Failure {
                    restore_idle: 100,
                    refund_shares: 100,
                },
            },
        )
        .unwrap();

        assert!(result.state.is_idle());
        assert_eq!(result.state.idle_assets, 500); // 400 + 100 restored
        assert_eq!(result.state.total_shares, 1_000); // Not changed
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::TransferShares { .. })
        ));
        let event = result
            .effects
            .iter()
            .find_map(|e| match e {
                KernelEffect::EmitEvent {
                    event:
                        KernelEvent::PayoutCompleted {
                            op_id,
                            success,
                            burn_shares,
                            refund_shares,
                            amount,
                        },
                } => Some((*op_id, *success, *burn_shares, *refund_shares, *amount)),
                _ => None,
            })
            .expect("missing PayoutCompleted event");
        assert_eq!(event, (13, false, 0, 100, 0));
    }

    #[test]
    fn settle_payout_wrong_state_fails() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 1,
                outcome: PayoutOutcome::Success {
                    burn_shares: 100,
                    refund_shares: 0,
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState("settle_payout requires Payout"))
        ));
    }

    #[test]
    fn settle_payout_op_id_mismatch_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 20,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 99,
                outcome: PayoutOutcome::Success {
                    burn_shares: 100,
                    refund_shares: 0,
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::OpIdMismatch {
                expected: 20,
                actual: 99
            })
        ));
    }

    #[test]
    fn settle_payout_empty_queue_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 20,
            owner: addr(1),
            receiver: addr(2),
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 20,
                outcome: PayoutOutcome::Success {
                    burn_shares: 100,
                    refund_shares: 0,
                },
            },
        );

        assert!(matches!(result, Err(KernelError::EmptyQueue)));
    }

    #[test]
    fn settle_payout_queue_head_mismatch_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();

        state
            .withdraw_queue
            .enqueue(
                addr(99),
                addr(99),
                100,
                100,
                0,
                config.max_pending_withdrawals,
            )
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 20,
            owner: addr(1),
            receiver: addr(2),
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 20,
                outcome: PayoutOutcome::Success {
                    burn_shares: 100,
                    refund_shares: 0,
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState("withdrawal queue head mismatch"))
        ));
    }

    #[test]
    fn settle_payout_success_settlement_mismatch_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 20,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 20,
                outcome: PayoutOutcome::Success {
                    burn_shares: 50,
                    refund_shares: 10, // 50 + 10 != 100 escrow
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "payout success settlement mismatch"
            ))
        ));
    }

    #[test]
    fn settle_payout_failure_settlement_mismatch_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 20,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 20,
                outcome: PayoutOutcome::Failure {
                    restore_idle: 100,
                    refund_shares: 50, // Should be 100
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "payout failure settlement mismatch"
            ))
        ));
    }

    #[test]
    fn settle_payout_failure_restore_idle_mismatch_fails() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 500, 500, 0);
        let config = test_config();
        let owner = addr(1);
        let receiver = addr(2);

        state
            .withdraw_queue
            .enqueue(owner, receiver, 100, 100, 0, config.max_pending_withdrawals)
            .unwrap();

        state.op_state = OpState::Payout(PayoutState {
            op_id: 21,
            owner,
            receiver,
            amount: 100,
            escrow_shares: 100,
            burn_shares: 100,
        });

        // restore_idle: 200 doesn't match payout.amount: 100
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::SettlePayout {
                op_id: 21,
                outcome: PayoutOutcome::Failure {
                    restore_idle: 200, // Should be 100
                    refund_shares: 100,
                },
            },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "payout failure restore_idle must equal payout.amount"
            ))
        ));
    }

    // =========================================================================
    // Pause action tests
    // =========================================================================

    #[test]
    fn pause_action() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::Pause { paused: true },
        )
        .unwrap();

        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::PauseUpdated { paused: true }
            })
        ));
    }

    // =========================================================================
    // RefreshFees action tests
    // =========================================================================

    #[test]
    fn refresh_fees_action_zero_fees() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config(); // fees: FeesSpec::zero()

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: 12345 },
        )
        .unwrap();

        assert_eq!(result.state.fee_anchor.total_assets, 1_000);
        assert_eq!(result.state.fee_anchor.timestamp_ns, 12345);
        assert_eq!(result.state.total_shares, 1_000); // No fee shares minted
        assert_eq!(result.effects.len(), 1); // Only FeesRefreshed event
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::FeesRefreshed { now_ns: 12345, .. }
            })
        ));
    }

    #[test]
    fn refresh_fees_mints_performance_fee_shares() {
        use crate::math::wad::YEAR_NS;
        // Setup: vault started with 1000 assets/shares, now has 1500 assets (profit)
        let mut state = VaultState::with_initial(1_500, 1_000, 1_500, 0, 0);
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0); // anchor at 1000 assets, time 0

        let perf_recipient = addr(0xAA);
        let mut config = test_config();
        config.fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance fee
            FeeSlot::zero(),                                 // no management fee
            None,
        );

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: YEAR_NS },
        )
        .unwrap();

        // Profit = 1500 - 1000 = 500; fee_assets = 10% * 500 = 50
        // denom = 1500 - 50 = 1450; perf_shares = floor(50 * 1000 / 1450) = 34
        let mint_effects: Vec<_> = result
            .effects
            .iter()
            .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
            .collect();
        assert_eq!(mint_effects.len(), 1);
        assert!(matches!(
            mint_effects[0],
            KernelEffect::MintShares { owner, shares: 34 } if *owner == perf_recipient
        ));
        assert_eq!(result.state.total_shares, 1_000 + 34);
    }

    #[test]
    fn refresh_fees_mints_management_fee_shares() {
        use crate::math::wad::YEAR_NS;
        // Setup: 1000 assets/shares, no profit, full year elapsed
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

        let mgmt_recipient = addr(0xBB);
        let mut config = test_config();
        config.fees = FeesSpec::new(
            FeeSlot::zero(),                                   // no performance fee
            FeeSlot::new(Wad::one() / 10, mgmt_recipient),    // 10% management fee
            None,
        );

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: YEAR_NS },
        )
        .unwrap();

        // Full year: annual_fee_assets = 10% * 1000 = 100
        // fee_assets = floor(100 * YEAR_NS / YEAR_NS) = 100
        // fee_shares = floor(100 * 1000 / (1000 - 100)) = floor(100000/900) = 111
        let mint_effects: Vec<_> = result
            .effects
            .iter()
            .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
            .collect();
        assert_eq!(mint_effects.len(), 1);
        assert!(matches!(
            mint_effects[0],
            KernelEffect::MintShares { owner, shares: 111 } if *owner == mgmt_recipient
        ));
        assert_eq!(result.state.total_shares, 1_000 + 111);
    }

    #[test]
    fn refresh_fees_mints_both_management_and_performance() {
        use crate::math::wad::YEAR_NS;
        use crate::math::wad::compute_fee_shares_from_assets;

        let mut state = VaultState::with_initial(1_500, 1_000, 1_500, 0, 0);
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

        let perf_recipient = addr(0xAA);
        let mgmt_recipient = addr(0xBB);
        let mut config = test_config();
        config.fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, perf_recipient),  // 10% performance
            FeeSlot::new(Wad::one() / 20, mgmt_recipient),  // 5% management
            None,
        );

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: YEAR_NS },
        )
        .unwrap();

        // Management first: annual_fee_assets = 5% * 1500 = 75
        // mgmt_shares = floor(75 * 1000 / (1500 - 75)) = floor(75000/1425) = 52
        let mgmt_expected: u128 = compute_fee_shares_from_assets(
            Number::from(75u128),
            Number::from(1_500u128),
            Number::from(1_000u128),
        )
        .into();

        // Performance: supply now = 1000 + mgmt_expected; profit = 500; fee_assets = 50
        let total_supply_after_mgmt = 1_000 + mgmt_expected;
        let perf_expected: u128 = compute_fee_shares_from_assets(
            Number::from(50u128), // 10% of 500 profit
            Number::from(1_500u128),
            Number::from(total_supply_after_mgmt),
        )
        .into();

        let mint_effects: Vec<_> = result
            .effects
            .iter()
            .filter_map(|e| match e {
                KernelEffect::MintShares { owner, shares } => Some((*owner, *shares)),
                _ => None,
            })
            .collect();
        assert_eq!(mint_effects.len(), 2);
        assert_eq!(mint_effects[0], (mgmt_recipient, mgmt_expected));
        assert_eq!(mint_effects[1], (perf_recipient, perf_expected));
        assert_eq!(
            result.state.total_shares,
            1_000 + mgmt_expected + perf_expected
        );
    }

    #[test]
    fn refresh_fees_no_profit_skips_performance() {
        use crate::math::wad::YEAR_NS;
        // No profit (assets unchanged from anchor)
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

        let perf_recipient = addr(0xAA);
        let mut config = test_config();
        config.fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance
            FeeSlot::zero(),
            None,
        );

        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: YEAR_NS },
        )
        .unwrap();

        let mint_effects: Vec<_> = result
            .effects
            .iter()
            .filter(|e| matches!(e, KernelEffect::MintShares { .. }))
            .collect();
        assert_eq!(mint_effects.len(), 0);
        assert_eq!(result.state.total_shares, 1_000);
    }

    #[test]
    fn refresh_fees_max_rate_caps_fee_accrual() {
        use crate::math::wad::YEAR_NS;
        // 1000 -> 2000 (100% profit), but max_rate = 20% per year
        let mut state = VaultState::with_initial(2_000, 1_000, 2_000, 0, 0);
        state.fee_anchor = FeeAccrualAnchor::new(1_000, 0);

        let perf_recipient = addr(0xAA);
        let mut config = test_config();
        config.fees = FeesSpec::new(
            FeeSlot::new(Wad::one() / 10, perf_recipient), // 10% performance
            FeeSlot::zero(),
            Some(Wad::one() / 5), // 20% max growth rate
        );

        // Half year elapsed
        let half_year = YEAR_NS / 2;
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: half_year },
        )
        .unwrap();

        // Max growth = 1000 * 20% * 0.5 = 100; capped total_assets = 1000 + 100 = 1100
        // Profit = 1100 - 1000 = 100; fee_assets = 10% * 100 = 10
        // denom = 2000 - 10 = 1990; perf_shares = floor(10 * 1000 / 1990) = 5
        let mint_effects: Vec<_> = result
            .effects
            .iter()
            .filter_map(|e| match e {
                KernelEffect::MintShares { shares, .. } => Some(*shares),
                _ => None,
            })
            .collect();
        assert_eq!(mint_effects.len(), 1);
        assert_eq!(mint_effects[0], 5);
    }

    #[test]
    fn refresh_fees_rejects_backwards_time() {
        let mut state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        state.fee_anchor.timestamp_ns = 10000; // Current anchor at 10000
        let config = test_config();

        // Try to refresh with earlier timestamp
        let result = apply_action(
            state,
            &config,
            None,
            &addr(0xFF),
            KernelAction::RefreshFees { now_ns: 5000 },
        );

        assert!(matches!(
            result,
            Err(KernelError::InvalidState(
                "fee refresh timestamp cannot go backwards"
            ))
        ));
    }

    // =========================================================================
    // Helper function tests
    // =========================================================================

    #[test]
    fn effective_totals_adds_virtual() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let mut config = test_config();
        config.virtual_shares = 100;
        config.virtual_assets = 200;

        let (supply, assets) = effective_totals(&state, &config);
        assert_eq!(supply, 1_000 + 100); // shares + max(virtual, 1)
        assert_eq!(assets, 1_000 + 200); // assets + max(virtual, 1)
    }

    #[test]
    fn convert_to_shares_works() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        // With 1:1 ratio (plus virtual adjustments)
        let shares = convert_to_shares(&state, &config, 500);
        // shares = 500 * (1001) / (1001) = 500
        assert_eq!(shares, 500);
    }

    #[test]
    fn convert_to_assets_works() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

        let assets = convert_to_assets(&state, &config, 500);
        assert_eq!(assets, 500);
    }

    #[test]
    fn kernel_result_new() {
        let state = VaultState::new();
        let effects = vec![KernelEffect::EmitEvent {
            event: KernelEvent::PauseUpdated { paused: false },
        }];

        let result = KernelResult::new(state.clone(), effects.clone());
        assert_eq!(result.state, state);
        assert_eq!(result.effects, effects);
    }
}
