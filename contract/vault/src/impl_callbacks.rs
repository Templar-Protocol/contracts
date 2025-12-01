#![allow(clippy::too_many_arguments)]

use core::cmp::Ordering;
use std::fmt::Display;

use crate::{
    governance::Gate, near, Contract, ContractExt, Error, EscrowSettlement, Nep141Controller,
    OpState,
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, json_types::U128, AccountId, Gas, NearToken, PromiseError, PromiseOrValue};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Transfer};
use templar_common::{
    market::ext_market,
    supply::SupplyPosition,
    vault::{
        AllocatingState, Event, IdleBalanceDelta, PayoutState, WithdrawingState,
        EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS, EXECUTE_WITHDRAW_03_SETTLE_GAS,
        GET_SUPPLY_POSITION_GAS, SUPPLY_02_POSITION_READ_GAS,
    },
};

/// State machine:
///
/// - Allocating -> Withdrawing (or Idle via stop)
/// - Withdrawing -> Withdrawing (advance) | Payout | Idle (refund)
/// - Payout -> Idle (success or failure)
///
/// Invariants:
/// - idle_balance increases only when funds are received and is pre-decremented when payout is initiated (restored on failure).
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

    #[allow(clippy::too_many_arguments)]
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
        let (ctx, market) = match self.withdraw_ctx_and_market_or_exit(op_id, market_index) {
            Ok(v) => v,
            Err(p) => return p,
        };

        if did_create.is_ok() {
            self.pending_market_exec.push(market_index);
            PromiseOrValue::Value(())
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
                collected: ctx.collected,
                owner: ctx.owner.clone(),
                escrow_shares: ctx.escrow_shares,
            });
            self.step_withdraw()
        }
    }

    #[private]
    pub fn execute_withdraw_01_call_market_fetch_position(
        &mut self,
        #[callback_result] before_balance: Result<U128, PromiseError>,
        op_id: u64,
        market_index: u32,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        let (_ctx, market) = match self.withdraw_ctx_and_market_or_exit(op_id, market_index) {
            Ok(v) => v,
            Err(p) => return p,
        };

        let principal = self.principal_of(&market);
        let before_balance = before_balance.unwrap_or(U128(0));

        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(Gas::from_tgas(
                    EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS.as_tgas()
                        * (u64::from(batch_limit.unwrap_or(1))),
                ))
                .with_unused_gas_weight(0)
                .execute_next_supply_withdrawal_request(batch_limit)
                .then(
                    ext_market::ext(market.clone())
                        .with_static_gas(GET_SUPPLY_POSITION_GAS)
                        .with_unused_gas_weight(0)
                        .get_supply_position(env::current_account_id()),
                )
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(EXECUTE_WITHDRAW_03_SETTLE_GAS)
                        .execute_withdraw_02_reconcile_position(
                            op_id,
                            market_index,
                            U128(principal),
                            before_balance,
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
    ///
    /// # Panics
    /// - If the market is not found.
    #[private]
    pub fn execute_withdraw_02_reconcile_position(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_index: u32,
        principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let (_ctx, market) = match self.withdraw_ctx_and_market_or_exit(op_id, market_index) {
            Ok(v) => v,
            Err(p) => return p,
        };

        let reported_principal: u128 = match position {
            Ok(Some(position)) => position.get_deposit().total().into(),
            Ok(None) => {
                Event::WithdrawalPositionMissing {
                    op_id: op_id.into(),
                    market: market.clone(),
                    index: market_index,
                    before: principal,
                }
                .emit();
                // Treat missing position as zero principal and continue to balance settlement
                0
            }
            Err(_) => {
                Event::WithdrawalPositionReadFailed {
                    op_id: op_id.into(),
                    market: market.clone(),
                    index: market_index,
                    before: principal,
                }
                .emit();
                return self.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(Gas::from_tgas(5))
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(EXECUTE_WITHDRAW_03_SETTLE_GAS)
                        .execute_withdraw_03_settle(
                            op_id,
                            market_index,
                            principal,
                            U128(reported_principal),
                            before_balance,
                        ),
                ),
        )
    }

    #[allow(clippy::too_many_lines)]
    #[private]
    pub fn execute_withdraw_03_settle(
        &mut self,
        #[callback_result] after_balance: Result<U128, PromiseError>,
        op_id: u64,
        market_index: u32,
        before_principal: U128,
        reported_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let (ctx, market) = match self.withdraw_ctx_and_market_or_exit(op_id, market_index) {
            Ok(v) => v,
            Err(p) => return p,
        };

        let (principal_delta, inflow, creditable) = Self::compute_withdraw_deltas(
            before_principal,
            reported_principal,
            after_balance,
            before_balance,
        );
        let extra = inflow.saturating_sub(principal_delta);

        match principal_delta.cmp(&inflow) {
            Ordering::Greater => {
                Event::WithdrawalInflowMismatch {
                    op_id: op_id.into(),
                    market: market.clone(),
                    index: market_index,
                    delta: U128(principal_delta),
                    inflow: U128(inflow),
                }
                .emit();
            }
            Ordering::Less => {
                Event::WithdrawalOverpayCredited {
                    op_id: op_id.into(),
                    market: market.clone(),
                    index: market_index,
                    extra: U128(extra),
                }
                .emit();
            }
            Ordering::Equal => {}
        }

        let effective_principal = before_principal.0.saturating_sub(creditable);

        if let Some(rec) = self.markets.get_mut(&market) {
            rec.principal = effective_principal;
        }
        if inflow > 0 {
            self.update_idle_balance(IdleBalanceDelta::Increase(inflow.into()));
        }

        self.try_settle_pending_market_exec(market_index, creditable, principal_delta);

        // Reconcile remaining/collected based on credited inflow only
        let WithdrawReconciliation {
            remaining_next,
            collected_next,
            ..
        } = reconcile_withdraw_outcome(
            before_principal.0,
            effective_principal,
            ctx.remaining,
            ctx.collected,
        );

        // If market overpaid beyond principal drop, use the extra to satisfy this withdrawal
        let extra_payout = extra.min(remaining_next);
        let remaining_next = remaining_next.saturating_sub(extra_payout);
        let collected_next = collected_next.saturating_add(extra_payout);

        if remaining_next == 0 {
            return self.pay_collected(
                op_id,
                &ctx.receiver,
                collected_next,
                &ctx.owner,
                ctx.escrow_shares,
                ctx.escrow_shares,
                |self_| {
                    // On early completion we still finalise
                    let self_id = env::current_account_id();
                    self_
                        .transfer(&Nep141Transfer::new(
                            ctx.escrow_shares,
                            &self_id,
                            &ctx.owner,
                        ))
                        .unwrap_or_else(|e| {
                            templar_common::panic_with_message(&format!(
                                "Failed to refund escrowed shares {e}"
                            ))
                        });
                    self_.pending_market_exec.clear();
                    self_.remove_inflight_and_advance_head();
                    self_.withdraw_route.clear();
                    self_.op_state = OpState::Idle;
                    PromiseOrValue::Value(())
                },
            );
        }

        match principal_delta.cmp(&inflow) {
            Ordering::Less | Ordering::Equal if principal_delta > 0 => {
                // Fully executed for this market: advance to next and continue
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
            _ => {
                // Partial or zero inflow: do not advance; keeper must re-execute this market later
                self.op_state = OpState::Withdrawing(WithdrawingState {
                    op_id,
                    index: market_index,
                    remaining: remaining_next,
                    receiver: ctx.receiver,
                    collected: collected_next,
                    owner: ctx.owner,
                    escrow_shares: ctx.escrow_shares,
                });
                PromiseOrValue::Value(())
            }
        }
    }
    /// Cash flow:
    /// - Runs in Payout context after funds were credited in after_exec_withdraw_read.
    /// - On success: idle_balance was pre-decremented before transfer; burn a portion of escrow_shares and refund the rest to the owner.
    /// - On failure: refund full escrow_shares to the owner and restore idle_balance (funds remain in vault).
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
            let EscrowSettlement {
                to_burn: burn_shares,
                refund,
            } = Self::compute_escrow_settlement(escrow_shares, burn_shares);

            // Burn only the proportional shares and refund the remainder to the owner.
            if burn_shares > 0 {
                // Serious issue: this should be infallible - if the withdrawal panics here we have an escrow settlement error
                let _ = self
                    .burn(&Nep141Burn::new(burn_shares, env::current_account_id()))
                    .inspect_err(|e| env::log_str(&format!("Failed to burn {e}")));
            }

            if refund > 0 {
                // Note: this should be infallible since we are transferring to an existing owner, and they are unable to unregister from storage
                Gate::bypass_transfer_with(
                    self,
                    &Nep141Transfer::new(refund, env::current_account_id(), &owner),
                    // Serious issue: this should be infallible - if the transfer panics here we have an escrow settlement error
                    |e| env::log_str(&e.to_string()),
                );
            }
        } else {
            // On payout failure, refund full escrow to owner and restore idle_balance
            self.update_idle_balance(IdleBalanceDelta::Increase(expected_amount.into()));
            Gate::bypass_transfer_with(
                self,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner),
                |e| env::log_str(&e.to_string()),
            );
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
                .with_static_gas(Gas::from_tgas(5))
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

        self.update_idle_balance(IdleBalanceDelta::Increase(s.remaining.into()));

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

    /// Combined helper for withdrawing callbacks: validate ctx and resolve market.
    /// Returns (cloned context, owned market `AccountId`) on success, or calls `stop_and_exit` and returns Err on failure.
    pub(crate) fn withdraw_ctx_and_market_or_exit(
        &mut self,
        op_id: u64,
        market_index: u32,
    ) -> Result<(WithdrawingState, AccountId), PromiseOrValue<()>> {
        let ctx = match self.ctx_withdrawing(op_id) {
            Ok(s) => s.clone(),
            Err(e) => return Err(self.stop_and_exit(Some(&e))),
        };

        if ctx.index != market_index {
            return Err(self.stop_and_exit(Some(&Error::IndexDrifted(ctx.index, market_index))));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m.clone(),
            Err(e) => return Err(self.stop_and_exit(Some(&e))),
        };

        Ok((ctx, market))
    }

    /// Resolve a market for withdraw by `withdraw_route`
    pub(crate) fn resolve_withdraw_market(&self, market_index: u32) -> Result<&AccountId, Error> {
        self.withdraw_route
            .get(market_index as usize)
            .ok_or(Error::MissingMarket(market_index))
    }

    // Settle pending market exec entry only if fully credited
    pub fn try_settle_pending_market_exec(
        &mut self,
        market_index: u32,
        creditable: u128,
        principal_drop: u128,
    ) {
        if let Some(pos) = self
            .pending_market_exec
            .iter()
            .position(|&idx| idx == market_index)
        {
            if creditable == principal_drop {
                self.pending_market_exec.remove(pos);
            }
        }
    }

    #[must_use]
    pub fn compute_withdraw_deltas(
        before_principal: U128,
        new_principal_reported: U128,
        after_balance: Result<U128, PromiseError>,
        before_balance: U128,
    ) -> (u128, u128, u128) {
        // Principal drop as reported by the market
        let principal_delta = before_principal.0.saturating_sub(new_principal_reported.0);

        let after_balance = match after_balance {
            Ok(U128(v)) => v,
            Err(_) => 0,
        };
        let inflow = after_balance.saturating_sub(before_balance.0);

        // Compute effective principal drop we can book (conservative on shortfall)
        let creditable = principal_delta.min(inflow);
        (principal_delta, inflow, creditable)
    }

    pub fn update_idle_balance(&mut self, delta: IdleBalanceDelta) {
        let idle_balance = self.idle_balance;
        self.idle_balance = delta.apply(idle_balance);
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
