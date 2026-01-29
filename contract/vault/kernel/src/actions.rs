//! Kernel action dispatch for vault state transitions.
//!
//! This module defines the public `KernelAction` enum and a dispatcher that
//! applies actions to `VaultState` and returns effects.

extern crate alloc;

use crate::effects::KernelEffect;
use crate::error::KernelError;
use crate::math::number::Number;
use alloc::vec;
use alloc::vec::Vec;
use crate::math::wad::mul_div_floor;
use crate::restrictions::Restrictions;
use crate::state::queue::{is_past_cooldown, DEFAULT_COOLDOWN_NS};
use crate::state::vault::{FeeAccrualAnchor, VaultConfig, VaultState};
use crate::transitions::{
    complete_allocation, complete_refresh, start_allocation, start_refresh, start_withdrawal,
    stop_withdrawal, withdrawal_step_callback, WithdrawalRequest,
};
use crate::types::{Address, TimestampNs};
use crate::state::op_state::{OpState, TargetId};
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
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelAction {
    BeginAllocating {
        op_id: u64,
        plan: Vec<(TargetId, u128)>,
        now_ns: TimestampNs,
    },
    Deposit {
        owner: Address,
        receiver: Address,
        assets_in: u128,
        min_shares_out: u128,
        now_ns: TimestampNs,
    },
    RequestWithdraw {
        owner: Address,
        receiver: Address,
        shares: u128,
        min_assets_out: u128,
        now_ns: TimestampNs,
    },
    ExecuteWithdraw {
        now_ns: TimestampNs,
    },
    BeginRefreshing {
        op_id: u64,
        plan: Vec<TargetId>,
        now_ns: TimestampNs,
    },
    FinishAllocating {
        op_id: u64,
        now_ns: TimestampNs,
    },
    SyncExternalAssets {
        new_external_assets: u128,
        op_id: u64,
        now_ns: TimestampNs,
    },
    FinishRefreshing {
        op_id: u64,
        now_ns: TimestampNs,
    },
    AbortRefreshing {
        op_id: u64,
    },
    SettlePayout {
        op_id: u64,
        outcome: PayoutOutcome,
    },
    AbortAllocating {
        op_id: u64,
        restore_idle: u128,
    },
    AbortWithdrawing {
        op_id: u64,
        refund_shares: u128,
    },
    RefreshFees {
        now_ns: TimestampNs,
    },
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
                .enqueue(owner, receiver, shares, expected_assets, now_ns, config.max_pending_withdrawals)
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

                let result =
                    start_withdrawal(state.op_state.clone(), request).map_err(KernelError::Transition)?;
                state.op_state = result.new_state;

                Ok(KernelResult::new(state, result.effects))
            }
            OpState::Withdrawing(_) => {
                let Some((_, pending_ref)) = state.withdraw_queue.head() else {
                    return Err(KernelError::EmptyQueue);
                };
                let withdraw = match &state.op_state {
                    OpState::Withdrawing(s) => s,
                    _ => {
                        return Err(KernelError::InvalidState(
                            "execute_withdraw requires Withdrawing",
                        ))
                    }
                };

                if pending_ref.owner != withdraw.owner
                    || pending_ref.receiver != withdraw.receiver
                    || pending_ref.escrow_shares != withdraw.escrow_shares
                {
                    return Err(KernelError::InvalidState(
                        "withdrawal queue head mismatch",
                    ));
                }

                enforce_restrictions(config, restrictions, self_id, &withdraw.owner)?;
                enforce_restrictions(config, restrictions, self_id, &withdraw.receiver)?;

                let result =
                    withdrawal_step_callback(state.op_state.clone(), withdraw.op_id, 0)
                        .map_err(KernelError::Transition)?;
                state.op_state = result.new_state;
                Ok(KernelResult::new(state, result.effects))
            }
            _ => Err(KernelError::InvalidState(
                "execute_withdraw requires Idle or Withdrawing",
            )),
        },
        KernelAction::BeginAllocating { op_id, plan, .. } => {
            let result =
                start_allocation(state.op_state.clone(), plan, op_id).map_err(KernelError::Transition)?;
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
            let result =
                start_refresh(state.op_state.clone(), plan, op_id).map_err(KernelError::Transition)?;
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
                return Err(KernelError::InvalidState("sync_external_assets requires active op"));
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
            let current_op_id = state
                .op_state
                .op_id()
                .ok_or(KernelError::InvalidState("abort_refreshing requires active op"))?;
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
        KernelAction::AbortAllocating { op_id, restore_idle } => {
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
                return Err(KernelError::InvalidState(
                    "withdrawal queue head mismatch",
                ));
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
                _ => {
                    return Err(KernelError::InvalidState(
                        "settle_payout requires Payout",
                    ))
                }
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
                return Err(KernelError::InvalidState(
                    "withdrawal queue head mismatch",
                ));
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
                            owner: payout.owner,
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
    use alloc::string::String;
    use crate::effects::KernelEvent;
    use crate::fee::{Fee, Fees};
    use crate::math::wad::Wad;
    use crate::state::op_state::WithdrawingState;
    use crate::state::queue::DEFAULT_COOLDOWN_NS;

    fn addr(tag: u8) -> Address {
        [tag; 32]
    }

    fn test_config() -> VaultConfig {
        VaultConfig {
            fees: Fees {
                performance: Fee {
                    fee: Wad::ZERO,
                    recipient: String::new(),
                },
                management: Fee {
                    fee: Wad::ZERO,
                    recipient: String::new(),
                },
                max_total_assets_growth_rate: None,
            },
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
        )
        .unwrap();

        let withdraw = result.state.op_state.as_withdrawing().unwrap();
        assert_eq!(withdraw.op_id, 7);
        assert_eq!(withdraw.index, 1);
        assert_eq!(withdraw.collected, 0);
        assert_eq!(withdraw.remaining, 200);
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
}
