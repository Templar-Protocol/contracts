use std::fmt::Display;

use crate::{near, Contract, ContractExt, Error, EscrowSettlement, Nep141Controller, OpState};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, json_types::U128, AccountId, NearToken, PromiseError, PromiseOrValue};
use near_sdk_contract_tools::ft::{nep141::GAS_FOR_FT_TRANSFER_CALL, Nep141Burn, Nep141Transfer};
use templar_common::{
    market::ext_market,
    supply::SupplyPosition,
    vault::{
        AllocatingState, Event, PayoutState, WithdrawingState,
        EXECUTE_WITHDRAW_02_RECONCILE_POSITION_GAS, GET_SUPPLY_POSITION_GAS,
        SUPPLY_02_POSITION_READ_GAS,
    },
};

/// State machine:
///
/// - Allocating -> Withdrawing (or Idle via stop)
/// - Withdrawing -> Withdrawing (advance) | Payout | Idle (refund)
/// - Payout -> Idle (success or failure)
///
/// Invariants:
/// - idle_balance increases only when funds are received and decreases only on payout success.
/// - escrow_shares are refunded on stop/failure or partially burned/refunded on payout success.
#[near]
impl Contract {
    #[private]
    pub fn supply_01_handle_transfer(
        &mut self,
        #[callback_result] accepted: Result<U128, PromiseError>,
        market: AccountId,
        op_id: u64,
        market_index: u32,
        attempted: U128,
        remaining_before: U128,
    ) -> PromiseOrValue<()> {
        if let Err(e) = self.ctx_allocating(op_id) {
            return self.stop_and_exit(Some(&e));
        };

        match accepted {
            Err(_) => {
                Event::AllocationTransferFailed {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market.clone(),
                    attempted,
                }
                .emit();
                self.stop_and_exit(Some(&Error::MarketTransferFailed))
            }
            Ok(accepted) => {
                let before = self.principal_of(&market);

                PromiseOrValue::Promise(
                    ext_market::ext(market.clone())
                        .with_static_gas(GET_SUPPLY_POSITION_GAS)
                        .with_unused_gas_weight(0)
                        .get_supply_position(env::current_account_id())
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(SUPPLY_02_POSITION_READ_GAS)
                                .supply_02_position_read(
                                    market.clone(),
                                    op_id,
                                    market_index,
                                    U128(before),
                                    attempted,
                                    accepted,
                                    remaining_before,
                                ),
                        ),
                )
            }
        }
    }

    #[private]
    pub fn supply_02_position_read(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        market: AccountId,
        op_id: u64,
        market_index: u32,
        before: U128,
        attempted: U128,
        accepted: U128,
        remaining_before: U128,
    ) -> PromiseOrValue<()> {
        let (i, _remaining_ctx) = match self.ctx_allocating(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if i != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(i, market_index)));
        }

        let SupplyReconciliation {
            new_principal,
            accepted_event,
            remaining: remaining_next,
        } = match position {
            Ok(Some(position)) => reconcile_supply_outcome(
                &position.get_deposit().total().into(),
                &before.0,
                &remaining_before.0,
            ),
            Ok(None) => {
                Event::AllocationPositionMissing {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market.clone(),
                    attempted,
                    accepted,
                }
                .emit();
                return self.stop_and_exit(Some(&Error::MissingSupplyPosition));
            }
            Err(_) => {
                Event::AllocationPositionReadFailed {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market.clone(),
                    attempted,
                    accepted,
                }
                .emit();
                return self.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        let refunded = attempted.0.saturating_sub(accepted_event);
        Event::AllocationStepSettled {
            op_id: op_id.into(),
            index: market_index,
            market: market.clone(),
            before,
            new_principal: U128(new_principal),
            accepted: U128(accepted_event),
            attempted,
            refunded: U128(refunded),
            remaining_after: U128(remaining_next),
        }
        .emit();

        if let Some(rec) = self.markets.get_mut(&market) {
            rec.principal = new_principal;
        }

        self.op_state = OpState::Allocating(AllocatingState {
            op_id,
            index: market_index.saturating_add(1),
            remaining: remaining_next,
        });
        if remaining_next == 0 {
            // All funds allocated successfully
            return self.stop_and_exit(None::<&String>);
        }
        self.step_allocation()
    }

    #[private]
    pub fn withdraw_01_handle_create_request(
        &mut self,
        #[callback_result] did_create: Result<(), PromiseError>,
        op_id: u64,
        market_index: u32,
        need: U128,
    ) -> PromiseOrValue<()> {
        let ctx = match self.ctx_withdrawing(op_id) {
            Ok(s) => s,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if ctx.index != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(ctx.index, market_index)));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if did_create.is_ok() {
            // Always defer execution: record the created request; keeper must call execute_next_market_withdrawal(op_id)
            self.pending_market_exec.push(market_index);
            return PromiseOrValue::Value(());
        } else {
            Event::CreateWithdrawalFailed {
                op_id: op_id.into(),
                market: market.clone(),
                index: ctx.index,
                need,
            }
            .emit();
            self.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                index: market_index.saturating_add(1),
                remaining: ctx.remaining,
                receiver: ctx.receiver.clone(),
                collected: ctx.collected.clone(),
                owner: ctx.owner.clone(),
                escrow_shares: ctx.escrow_shares,
            });
            self.step_withdraw()
        }
    }

    #[private]
    pub fn execute_withdraw_01_fetch_position(
        &mut self,
        op_id: u64,
        market_index: u32,
        need: U128,
    ) -> PromiseOrValue<()> {
        let ctx = match self.ctx_withdrawing(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if ctx.index != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(ctx.index, market_index)));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        // Verify actual withdrawal by reading market position after execution
        let before = self.principal_of(market);
        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(GET_SUPPLY_POSITION_GAS)
                .with_unused_gas_weight(0)
                .get_supply_position(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(EXECUTE_WITHDRAW_02_RECONCILE_POSITION_GAS)
                        .execute_withdraw_02_reconcile_position(
                            op_id,
                            market_index,
                            U128(before),
                            need,
                        ),
                ),
        )
    }

    /// Cash flow:
    /// - Reconcile market position to compute 'credited' (funds returned from market).
    /// - Increment idle_balance by credited to reflect funds now held by the vault.
    /// - If remaining == 0, transition to Payout; otherwise continue Withdrawing on next market.
    /// - Later in after_send_to_user, idle_balance is decremented on successful transfer to the user.
    /// - On transfer failure, idle_balance stays unchanged and escrowed shares are refunded to the owner.
    #[private]
    pub fn execute_withdraw_02_reconcile_position(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_index: u32,
        before: U128,
        need: U128,
    ) -> PromiseOrValue<()> {
        let ctx = match self.ctx_withdrawing(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        }
        .clone();

        if ctx.index != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(ctx.index, market_index)));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        let before_principal = before.0;
        let new_principal = match position {
            Ok(Some(position)) => {
                let np: u128 = position.get_deposit().total().into();
                np
            }
            Ok(None) => {
                // No position => treat as principal = 0
                0
            }
            Err(_) => {
                Event::WithdrawalPositionReadFailed {
                    op_id: op_id.into(),
                    market: market.clone(),
                    index: market_index,
                    before: U128(before_principal),
                    need,
                }
                .emit();
                before_principal
            }
        };

        let WithdrawReconciliation {
            remaining_next,
            collected_next,
            idle_delta,
            ..
        } = reconcile_withdraw_outcome(
            before_principal,
            new_principal,
            ctx.remaining,
            ctx.collected,
        );

        if let Some(rec) = self.markets.get_mut(&market.clone()) {
            rec.principal = new_principal;
        }
        if idle_delta > 0 {
            self.idle_balance = self.idle_balance.saturating_add(idle_delta);
        }

        if let Some(pos) = self
            .pending_market_exec
            .iter()
            .position(|&idx| idx == market_index)
        {
            self.pending_market_exec.remove(pos);
        }

        if remaining_next == 0 {
            self.pay_collected(
                op_id,
                &ctx.receiver,
                collected_next,
                &ctx.owner,
                ctx.escrow_shares,
                ctx.escrow_shares,
                |_self| {
                    // Nothing collected; refund escrowed shares
                    let self_id = env::current_account_id();
                    // We expect the owner to maintain storage accounts, otherwise they will lose access to their funds
                    _self
                        .transfer(&Nep141Transfer::new(
                            ctx.escrow_shares,
                            &self_id,
                            &ctx.owner,
                        ))
                        .expect("Failed to refund escrowed shares");
                    _self.op_state = OpState::Idle;
                    PromiseOrValue::Value(())
                },
            )
        } else {
            self.op_state = OpState::Withdrawing(WithdrawingState {
                op_id,
                index: market_index.saturating_add(1),
                remaining: remaining_next,
                receiver: ctx.receiver,
                collected: collected_next,
                owner: ctx.owner,
                escrow_shares: ctx.escrow_shares,
            });
            self.step_withdraw()
        }
    }

    /// Cash flow:
    /// - Runs in Payout context after funds were credited in after_exec_withdraw_read.
    /// - On success: idle_balance -= amount; burn a portion of escrow_shares and refund the rest to the owner.
    /// - On failure: refund full escrow_shares to the owner and keep idle_balance unchanged (funds remain in vault).
    #[private]
    pub fn payment_01_reconcile_idle_or_refund(
        &mut self,
        #[callback_result] result: Result<(), PromiseError>,
        op_id: u64,
        receiver: AccountId,
        amount: U128,
    ) {
        let (owner, escrow_shares, expected_amount, burn_shares) = match &self.op_state {
            OpState::Payout(PayoutState {
                op_id: current_op,
                receiver: recv,
                amount,
                owner,
                escrow_shares,
                burn_shares,
            }) if *current_op == op_id && *recv == receiver => {
                (owner.clone(), *escrow_shares, *amount, *burn_shares)
            }
            _ => {
                Event::PayoutUnexpectedState {
                    op_id: op_id.into(),
                    receiver: receiver.clone(),
                    amount,
                }
                .emit();
                return;
            }
        };

        if result.is_ok() {
            // On payout success, idle_balance -= payout_amount.
            self.idle_balance = self.idle_balance.saturating_sub(expected_amount);

            let EscrowSettlement {
                to_burn: burn_shares,
                refund,
            } = Self::compute_escrow_settlement(escrow_shares, burn_shares);

            // Burn only the proportional shares and refund the remainder to the owner.
            if burn_shares > 0 {
                // Serious issue: this should be infallible - if the withdrawal panics here we have an escrow settlement error
                self.burn(&Nep141Burn::new(burn_shares, &env::current_account_id()));
            }

            // Maybe refund any delta to the owner
            if refund > 0 {
                // Note: this should be infallible since we are transferring to an existing owner, and they are unable to unregister from storage
                self.transfer(&Nep141Transfer::new(
                    refund,
                    &env::current_account_id(),
                    &owner,
                ))
                // Serious issue: this should be infallible - if the transfer panics here we have an escrow settlement error
                .unwrap_or_else(|e| env::log_str(&e.to_string()));
            }
        } else {
            // On payout failure, refund full escrow to owner and leave idle_balance unchanged
            self.transfer(&Nep141Transfer::new(
                escrow_shares,
                &env::current_account_id(),
                &owner,
            ))
            // If this fails, this is a serious issue as above
            .unwrap_or_else(|e| env::log_str(&e.to_string()));
        }
        self.pending_market_exec.clear();
        self.remove_inflight_and_advance_head();
        self.withdraw_route.clear();
        self.op_state = OpState::Idle;
    }

    #[private]
    pub fn skim_01_read_balance(
        &mut self,
        #[callback_result] balance: Result<U128, PromiseError>,
        token: AccountId,
        recipient: AccountId,
    ) -> PromiseOrValue<()> {
        let amount = match balance {
            Ok(U128(v)) if v > 0 => v,
            _ => {
                // Invariant: Skim does nothing for zero balance
                Event::SkimNoop {
                    token: token.clone(),
                    recipient: recipient.clone(),
                }
                .emit();
                return PromiseOrValue::Value(());
            }
        };
        PromiseOrValue::Promise(
            ext_ft_core::ext(token)
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
                .ft_transfer(recipient, U128(amount), None),
        )
    }
}

impl Contract {
    pub fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &AllocatingState = self.op_state.as_ref();

        msg.map_or(Event::AllocationCompleted { op_id: s.op_id }, |m| {
            Event::AllocationStopped {
                op_id: s.op_id.into(),
                index: s.index,
                remaining: U128(s.remaining),
                reason: Some(m.to_string()),
            }
        })
        .emit();

        self.idle_balance = self.idle_balance.saturating_add(s.remaining);
        self.plan = None;
        self.op_state = OpState::Idle;
    }

    /// Stop helper for Withdrawing: refund escrowed shares to owner and go Idle.
    pub fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &WithdrawingState = self.op_state.as_ref();

        Event::WithdrawalStopped {
            op_id: s.op_id.into(),
            index: s.index,
            remaining: U128(s.remaining),
            collected: U128(s.collected),
            reason: msg.map(std::string::ToString::to_string),
        }
        .emit();

        let owner = s.owner.clone();

        if s.escrow_shares > 0 {
            #[allow(clippy::expect_used, reason = "No side effects")]
            self.transfer_unchecked(&env::current_account_id(), &owner, s.escrow_shares)
                .unwrap_or_else(|e| env::log_str(&e.to_string()));
        }

        self.remove_inflight_and_advance_head();
        self.withdraw_route.clear();
        self.op_state = OpState::Idle;
    }

    /// refund escrowed shares to owner and go Idle.
    pub fn stop_and_exit_payout<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &PayoutState = self.op_state.as_ref();
        Event::PayoutStopped {
            op_id: (s.op_id).into(),
            receiver: s.receiver.clone(),
            amount: U128(s.amount),
            reason: msg.map(std::string::ToString::to_string),
        }
        .emit();

        let owner = s.owner.clone();
        if s.escrow_shares > 0 {
            self.transfer_unchecked(&env::current_account_id(), &owner, s.escrow_shares)
                .unwrap_or_else(|e| env::log_str(&e.to_string()));
        }
        self.remove_inflight_and_advance_head();
        self.withdraw_route.clear();
        self.op_state = OpState::Idle;
    }

    pub(crate) fn stop_and_exit<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) -> PromiseOrValue<()> {
        match &self.op_state {
            OpState::Allocating(_) => self.stop_and_exit_allocating(msg),
            OpState::Withdrawing(_) => self.stop_and_exit_withdrawing(msg),
            OpState::Payout(_) => self.stop_and_exit_payout(msg),
            OpState::Idle => {
                Event::OperationStoppedWhileIdle {
                    reason: msg.map(std::string::ToString::to_string),
                }
                .emit();
            }
        }
        PromiseOrValue::Value(())
    }

    /// Validate current op is Allocating and return (index, remaining)
    pub(crate) fn ctx_allocating(&self, op_id: u64) -> Result<(u32, u128), Error> {
        match &self.op_state {
            OpState::Allocating(AllocatingState {
                op_id: cur,
                index,
                remaining,
            }) if *cur == op_id => Ok((*index, *remaining)),
            _ => Err(Error::NotAllocating),
        }
    }

    /// Validate current op is Withdrawing and return context tuple
    pub(crate) fn ctx_withdrawing(&self, op_id: u64) -> Result<&WithdrawingState, Error> {
        match &self.op_state {
            OpState::Withdrawing(s) if s.op_id == op_id => Ok(s),
            _ => Err(Error::NotWithdrawing),
        }
    }

    /// Resolve a market for withdraw by `withdraw_route`
    pub(crate) fn resolve_withdraw_market(&self, market_index: u32) -> Result<&AccountId, Error> {
        self.withdraw_route
            .get(market_index as usize)
            .ok_or(Error::MissingMarket(market_index))
    }
}

pub struct SupplyReconciliation {
    pub new_principal: u128,
    pub accepted_event: u128,
    pub remaining: u128,
}

#[must_use]
pub fn reconcile_supply_outcome(
    total_position: &u128,
    before: &u128,
    remaining: &u128,
) -> SupplyReconciliation {
    let accepted_event = total_position.saturating_sub(*before);
    let remaining = remaining.saturating_sub(accepted_event);
    SupplyReconciliation {
        new_principal: *total_position,
        accepted_event,
        remaining,
    }
}

pub struct WithdrawReconciliation {
    pub payout_delta: u128,
    pub remaining_next: u128,
    pub collected_next: u128,
    pub idle_delta: u128,
}

/// Pure reconciliation for withdraw read outcome to enable unit tests
#[must_use]
pub fn reconcile_withdraw_outcome(
    before_principal: u128,
    new_principal: u128,
    remaining_total: u128,
    collected_total: u128,
) -> WithdrawReconciliation {
    let withdrawn = before_principal.saturating_sub(new_principal);
    let idle_delta = withdrawn;
    let payout_delta = withdrawn.min(remaining_total);
    let remaining_next = remaining_total.saturating_sub(payout_delta);
    let collected_next = collected_total.saturating_add(payout_delta);
    WithdrawReconciliation {
        payout_delta,
        remaining_next,
        collected_next,
        idle_delta,
    }
}
