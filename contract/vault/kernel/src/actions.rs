//! Kernel action dispatch for vault state transitions.
//!
//! This module defines the public `KernelAction` enum and a dispatcher that
//! applies actions to `VaultState` and returns effects.

extern crate alloc;

use crate::effects::KernelEffect;
use crate::error::KernelError;
use crate::math::number::Number;
use crate::math::wad::mul_div_floor;
use crate::restrictions::Restrictions;
use crate::state::op_state::{OpState, TargetId};
use crate::state::queue::{is_past_cooldown, DEFAULT_COOLDOWN_NS};
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
    /// Transitions: Idle → Allocating
    BeginAllocating {
        op_id: u64,
        plan: Vec<(TargetId, u128)>,
        now_ns: TimestampNs,
    },

    /// Deposit assets into the vault and mint shares to the receiver.
    ///
    /// Requires: Idle state, non-zero assets, slippage check passes.
    Deposit {
        owner: Address,
        receiver: Address,
        assets_in: u128,
        min_shares_out: u128,
        now_ns: TimestampNs,
    },

    /// Request a withdrawal by escrowing shares in the queue.
    ///
    /// Requires: Idle state, non-zero shares, slippage and minimum checks pass.
    RequestWithdraw {
        owner: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    },

    /// Execute the next pending withdrawal from the queue.
    ///
    /// Transitions: Idle → Withdrawing, or advances Withdrawing state.
    ExecuteWithdraw { now_ns: TimestampNs },

    /// Begin refreshing external market balances.
    ///
    /// Transitions: Idle → Refreshing
    BeginRefreshing {
        op_id: u64,
        plan: Vec<TargetId>,
        now_ns: TimestampNs,
    },

    /// Complete an allocation operation.
    ///
    /// Transitions: Allocating → Idle or Allocating → Withdrawing (if pending).
    FinishAllocating { op_id: u64, now_ns: TimestampNs },

    /// Sync external asset balances during an active operation.
    ///
    /// Updates `external_assets` and `total_assets` accounting.
    SyncExternalAssets {
        new_external_assets: u128,
        op_id: u64,
        now_ns: TimestampNs,
    },

    /// Complete a refresh operation.
    ///
    /// Transitions: Refreshing → Idle
    FinishRefreshing { op_id: u64, now_ns: TimestampNs },

    /// Abort a refresh operation (e.g., on external call failure).
    ///
    /// Transitions: Refreshing → Idle
    AbortRefreshing { op_id: u64 },

    /// Settle a payout after asset transfer attempt.
    ///
    /// Transitions: Payout → Idle (burns/refunds shares based on outcome).
    SettlePayout { op_id: u64, outcome: PayoutOutcome },

    /// Abort an allocation operation (e.g., on external call failure).
    ///
    /// Transitions: Allocating → Idle (restores idle balance).
    AbortAllocating { op_id: u64, restore_idle: u128 },

    /// Abort a withdrawal operation (e.g., on external call failure).
    ///
    /// Transitions: Withdrawing → Idle (refunds escrowed shares).
    AbortWithdrawing { op_id: u64, refund_shares: u128 },

    /// Refresh fee calculations and mint fee shares.
    ///
    /// Accrues management and performance fees based on time and AUM growth.
    RefreshFees { now_ns: TimestampNs },

    /// Update the vault's paused state.
    ///
    /// When paused, deposits and withdrawals are blocked.
    Pause { paused: bool },
}

fn effective_totals(state: &VaultState, config: &VaultConfig) -> (u128, u128) {
    let supply = state
        .total_shares
        .saturating_add(config.virtual_shares)
        .saturating_add(1);
    let assets = state
        .total_assets
        .saturating_add(config.virtual_assets)
        .saturating_add(1);
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
                    to: [0u8; 32],
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
        KernelAction::ExecuteWithdraw { now_ns } => match state.op_state.clone() {
            OpState::Idle => {
                let Some((_, pending_ref)) = state.withdraw_queue.head() else {
                    return Err(KernelError::EmptyQueue);
                };
                let pending = pending_ref.clone();

                enforce_restrictions(config, restrictions, self_id, &pending.owner)?;
                enforce_restrictions(config, restrictions, self_id, &pending.receiver)?;

                if !is_past_cooldown(pending.requested_at_ns, now_ns, DEFAULT_COOLDOWN_NS) {
                    return Err(KernelError::Cooldown {
                        requested_at: pending.requested_at_ns,
                        now: now_ns,
                        cooldown_ns: DEFAULT_COOLDOWN_NS,
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

                let result = start_withdrawal(state.op_state.clone(), request)
                    .map_err(KernelError::Transition)?;
                state.op_state = result.new_state;

                Ok(KernelResult::new(state, result.effects))
            }
            OpState::Withdrawing(_) => Err(KernelError::InvalidState(
                "execute_withdraw requires Idle (use withdrawal callbacks to advance)",
            )),
            _ => Err(KernelError::InvalidState(
                "execute_withdraw requires Idle or Withdrawing",
            )),
        },
        KernelAction::BeginAllocating { op_id, plan, .. } => {
            let result = start_allocation(state.op_state.clone(), plan, op_id)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::FinishAllocating { op_id, now_ns } => {
            let pending = state.withdraw_queue.head().and_then(|(_, w)| {
                if is_past_cooldown(w.requested_at_ns, now_ns, DEFAULT_COOLDOWN_NS) {
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

            let result = complete_allocation(state.op_state.clone(), op_id, pending_req)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::BeginRefreshing { op_id, plan, .. } => {
            let result = start_refresh(state.op_state.clone(), plan, op_id)
                .map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::FinishRefreshing { op_id, .. } => {
            let result =
                complete_refresh(state.op_state.clone(), op_id).map_err(KernelError::Transition)?;
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

            state.external_assets = new_external_assets;
            state.total_assets = state.idle_assets.saturating_add(new_external_assets);

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

            let result =
                stop_withdrawal(state.op_state.clone(), op_id).map_err(KernelError::Transition)?;
            state.op_state = result.new_state;
            state.withdraw_queue.dequeue();
            Ok(KernelResult::new(state, result.effects))
        }
        KernelAction::SettlePayout { op_id, outcome } => {
            let payout = match &state.op_state {
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

            let escrow_address = [0u8; 32];
            let mut effects = Vec::new();

            match outcome {
                PayoutOutcome::Success {
                    burn_shares,
                    refund_shares,
                } => {
                    if burn_shares.saturating_add(refund_shares) != payout.escrow_shares {
                        return Err(KernelError::InvalidState(
                            "payout success settlement mismatch",
                        ));
                    }

                    if burn_shares > 0 {
                        effects.push(KernelEffect::BurnShares {
                            owner: escrow_address,
                            shares: burn_shares,
                        });
                        state.total_shares = state.total_shares.saturating_sub(burn_shares);
                    }
                    if refund_shares > 0 {
                        effects.push(KernelEffect::TransferShares {
                            from: escrow_address,
                            to: payout.owner,
                            shares: refund_shares,
                        });
                    }

                    state.op_state = OpState::Idle;
                }
                PayoutOutcome::Failure {
                    restore_idle,
                    refund_shares,
                } => {
                    if refund_shares != payout.escrow_shares {
                        return Err(KernelError::InvalidState(
                            "payout failure settlement mismatch",
                        ));
                    }

                    if refund_shares > 0 {
                        effects.push(KernelEffect::TransferShares {
                            from: escrow_address,
                            to: payout.owner,
                            shares: refund_shares,
                        });
                    }

                    state.idle_assets = state.idle_assets.saturating_add(restore_idle);
                    state.total_assets = state.idle_assets.saturating_add(state.external_assets);
                    state.op_state = OpState::Idle;
                }
            }

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
            state.fee_anchor = FeeAccrualAnchor::new(state.total_assets, now_ns);
            let total_assets = state.total_assets;
            Ok(KernelResult::new(
                state,
                vec![KernelEffect::EmitEvent {
                    event: crate::effects::KernelEvent::FeesRefreshed {
                        now_ns,
                        total_assets,
                    },
                }],
            ))
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
    use crate::fee::FeesSpec;
    use crate::state::op_state::WithdrawingState;
    use crate::state::queue::DEFAULT_COOLDOWN_NS;

    fn addr(tag: u8) -> Address {
        [tag; 32]
    }

    fn test_config() -> VaultConfig {
        VaultConfig {
            fees: FeesSpec::zero(),
            min_withdrawal_assets: 0,
            max_pending_withdrawals: 10,
            paused: false,
            virtual_shares: 0,
            virtual_assets: 0,
        }
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

        let mut state = VaultState::with_initial(1_000, 1_000, 300, 500, 0);
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
        assert_eq!(burn_owner, [0u8; 32]);
        assert_eq!(burn_shares, 100);
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
        assert_eq!(result.effects.len(), 2); // BurnShares + TransferShares
    }

    #[test]
    fn settle_payout_failure() {
        use crate::state::op_state::PayoutState;

        let mut state = VaultState::with_initial(1_000, 1_000, 400, 500, 0);
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
    fn refresh_fees_action() {
        let state = VaultState::with_initial(1_000, 1_000, 1_000, 0, 0);
        let config = test_config();

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
        assert!(matches!(
            result.effects.first(),
            Some(KernelEffect::EmitEvent {
                event: KernelEvent::FeesRefreshed { now_ns: 12345, .. }
            })
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
        assert_eq!(supply, 1_000 + 100 + 1); // shares + virtual + 1
        assert_eq!(assets, 1_000 + 200 + 1); // assets + virtual + 1
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
