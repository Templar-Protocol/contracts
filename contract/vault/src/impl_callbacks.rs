#![allow(clippy::too_many_arguments)]

use core::cmp::Ordering;
use std::fmt::Display;

use crate::{
    governance::Gate,
    near,
    op_guard::{AllocatingSpec, OpGuard, PayoutSpec, RefreshingSpec, WithdrawingSpec},
    Contract, ContractExt, Error, Nep141Controller, OpState, RealAssetsReport,
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env, json_types::U128, AccountId, Gas, NearToken, Promise, PromiseError, PromiseOrValue,
};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Transfer};
use templar_common::{
    guard::GuardSpec,
    market::ext_market,
    panic_with_message,
    supply::SupplyPosition,
    vault::{
        AllocatingState, AllocationPlan, AllocationPositionIssueKind, EscrowSettlement, Event,
        IdleBalanceDelta, MarketId, PayoutState, PositionReportOutcome, Reason,
        WithdrawalAccountingKind, WithdrawingState, AFTER_SEND_TO_USER_GAS,
        EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS, FT_BALANCE_OF_GAS, GET_SUPPLY_POSITION_GAS,
        SUPPLY_POSITION_READ_CALLBACK_GAS, WITHDRAW_SETTLE_CALLBACK_GAS,
    },
};

macro_rules! unwrap_or_return {
    ($expr:expr) => {{
        match $expr {
            Ok(value) => value,
            Err(return_value) => return return_value,
        }
    }};
}

pub(crate) use unwrap_or_return;

pub(crate) fn or_stop<'a, S>(
    contract: &'a mut Contract,
    op_id: u64,
) -> Result<OpGuard<'a, S>, PromiseOrValue<()>>
where
    S: GuardSpec<Contract, Error = Error>,
{
    if let Err(e) = S::validate(contract, Some(op_id)) {
        return Err(contract.stop_and_exit(Some(&e)));
    }

    let guard = OpGuard::<S>::expect(contract, Some(op_id)).unwrap_or_else(|e| {
        panic_with_message(&format!(
            "Invariant: guard validated but could not be built: {e}"
        ))
    });

    Ok(guard)
}

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
        market_id: MarketId,
        op_id: u64,
        market_index: u32,
        attempted: U128,
        remaining_before: U128,
    ) -> PromiseOrValue<()> {
        if let Err(e) = AllocatingSpec::validate(self, Some(op_id)) {
            return self.stop_and_exit(Some(&e));
        }

        match accepted {
            Err(_) => {
                Event::AllocationTransferFailed {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market_id,
                    attempted,
                }
                .emit();
                self.stop_and_exit(Some(&Error::MarketTransferFailed))
            }
            Ok(accepted) => {
                let before = self.principal_of(market_id);

                let market_account = self.market_account_by_id_or_panic(market_id).clone();

                PromiseOrValue::Promise(
                    ext_market::ext(market_account)
                        .with_static_gas(GET_SUPPLY_POSITION_GAS)
                        .with_unused_gas_weight(0)
                        .get_supply_position(env::current_account_id())
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(SUPPLY_POSITION_READ_CALLBACK_GAS)
                                .supply_02_position_read(
                                    market_id,
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
        market_id: MarketId,
        op_id: u64,
        market_index: u32,
        before: U128,
        attempted: U128,
        accepted: U128,
        remaining_before: U128,
    ) -> PromiseOrValue<()> {
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let current_index = allocating.state().index;
        if current_index != market_index {
            return allocating.stop_and_exit(Some(&Error::IndexDrifted(
                current_index,
                market_index,
            )));
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
                Event::AllocationPositionIssue {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market_id,
                    attempted,
                    accepted,
                    kind: AllocationPositionIssueKind::Missing,
                }
                .emit();

                return allocating.stop_and_exit(Some(&Error::MissingSupplyPosition));
            }
            Err(_) => {
                Event::AllocationPositionIssue {
                    op_id: op_id.into(),
                    index: market_index,
                    market: market_id,
                    attempted,
                    accepted,
                    kind: AllocationPositionIssueKind::ReadFailed,
                }
                .emit();
                return allocating.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        let refunded = attempted.0.saturating_sub(accepted_event);

        Event::AllocationStepSettled {
            op_id: op_id.into(),
            index: market_index,
            market: market_id,
            before,
            new_principal: U128(new_principal),
            accepted: U128(accepted_event),
            attempted,
            refunded: U128(refunded),
            remaining_after: U128(remaining_next),
        }
        .emit();

        let plan: AllocationPlan = allocating
            .state()
            .plan
            .iter()
            .filter(|m| m.0 != market_id)
            .cloned()
            .collect();

        allocating.set_market_principal(market_id, new_principal);

        let mut allocating = allocating.replace_state(AllocatingState {
            op_id,
            index: market_index.saturating_add(1),
            remaining: remaining_next,
            plan,
        });

        if remaining_next == 0 {
            return allocating.stop_and_exit(None::<&String>);
        }
        allocating.step_allocation()
    }

    #[private]
    pub fn withdraw_01_handle_create_request(
        &mut self,
        #[callback_result] did_create: Result<(), PromiseError>,
        op_id: u64,
        market: MarketId,
        need: U128,
    ) -> PromiseOrValue<()> {
        let _ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        if did_create.is_ok() {
            self.market_execution_lock.lock(market);
        } else {
            Event::CreateWithdrawalFailed {
                op_id: op_id.into(),
                market,
                need,
            }
            .emit();
        }
        PromiseOrValue::Value(())
    }

    /// Callback for allocator-only rebalance after attempting to create a
    /// market-side supply withdrawal request. On success, this only emits a
    /// diagnostic event and does not change op_state; on failure, it logs and
    /// leaves the vault Idle.
    #[private]
    pub fn rebalance_withdraw_01_after_create_request(
        &mut self,
        #[callback_result] did_create: Result<(), PromiseError>,
        market_id: MarketId,
        amount: U128,
    ) {
        match did_create {
            Ok(()) => Event::WithdrawRequestCreated {
                market: market_id,
                amount,
            }
            .emit(),
            Err(_) => {
                panic_with_message("Couldnt create withdraw request in market");
            }
        }
    }

    #[private]
    pub fn execute_withdraw_01_execute_withdraw_fetch_position(
        &mut self,
        #[callback_result] before_balance: Result<U128, PromiseError>,
        op_id: u64,
        market: MarketId,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        let _ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        let principal = self.principal_of(market);
        let before_balance = before_balance.unwrap_or(U128(self.idle_balance));

        Event::VaultBalance {
            amount: before_balance,
        }
        .emit();

        let market_account = self.market_account_by_id_or_panic(market).clone();

        PromiseOrValue::Promise(
            Self::market_execute_withdraw_and_fetch_position(market_account, batch_limit).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(WITHDRAW_SETTLE_CALLBACK_GAS)
                    .execute_withdraw_02_reconcile_position(
                        op_id,
                        market,
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
        market: MarketId,
        principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let _ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        let reported_principal: u128 = match position {
            Ok(Some(position)) => {
                Event::WithdrawPositionReport {
                    outcome: PositionReportOutcome::Ok,
                    op_id: op_id.into(),
                    market,
                    position: Some(position.clone()),
                    before: None,
                }
                .emit();
                position.get_deposit().total().into()
            }
            Ok(None) => {
                Event::WithdrawPositionReport {
                    outcome: PositionReportOutcome::Missing,
                    op_id: op_id.into(),
                    market,
                    position: None,
                    before: Some(principal),
                }
                .emit();
                // Treat missing position as zero principal and continue to balance settlement
                0
            }
            Err(_) => {
                Event::WithdrawPositionReport {
                    outcome: PositionReportOutcome::ReadFailed,
                    op_id: op_id.into(),
                    market,
                    position: None,
                    before: Some(principal),
                }
                .emit();

                return self.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        PromiseOrValue::Promise(
            ext_ft_core::ext(self.underlying_asset.contract_id().into())
                .with_static_gas(FT_BALANCE_OF_GAS)
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(WITHDRAW_SETTLE_CALLBACK_GAS)
                        .execute_withdraw_03_settle(
                            op_id,
                            market,
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
        market: MarketId,
        before_principal: U128,
        reported_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        let Ok(after_balance) = after_balance else {
            return self.stop_and_exit(Some(&Error::BalanceReadFailed));
        };

        let (principal_delta, inflow, creditable) = self.settle_market_principal_and_idle(
            market,
            before_principal,
            reported_principal,
            after_balance,
            before_balance,
        );
        let effective_principal = before_principal.0.saturating_sub(creditable);
        let extra = inflow.saturating_sub(principal_delta);

        match principal_delta.cmp(&inflow) {
            Ordering::Greater => {
                Event::WithdrawalAccounting {
                    kind: WithdrawalAccountingKind::InflowMismatch,
                    op_id: op_id.into(),
                    market,
                    delta: Some(U128(principal_delta)),
                    inflow: Some(U128(inflow)),
                    extra: None,
                }
                .emit();
            }
            Ordering::Less => {
                Event::WithdrawalAccounting {
                    kind: WithdrawalAccountingKind::OverpayCredited,
                    op_id: op_id.into(),
                    market,
                    delta: None,
                    inflow: None,
                    extra: Some(U128(extra)),
                }
                .emit();
            }
            Ordering::Equal => {}
        }

        self.market_execution_lock.unlock(market);

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
        let (_, remaining_next, collected_next) =
            determine_payout_delta(remaining_next, collected_next, extra);

        if remaining_next == 0 {
            return self.pay_or_else(
                op_id,
                &ctx.receiver,
                collected_next,
                &ctx.owner,
                ctx.escrow_shares,
                ctx.escrow_shares,
                |self_| {
                    let mut withdrawing =
                        unwrap_or_return!(or_stop::<WithdrawingSpec>(self_, op_id));

                    // On early completion we still finalise
                    let self_id = env::current_account_id();
                    withdrawing
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
                    withdrawing.pop_head();
                    withdrawing.withdraw_route.clear();
                    let _idle = withdrawing.into_idle();
                    PromiseOrValue::Value(())
                },
            );
        }

        let next_state = state_on_delta_inflow(
            principal_delta,
            inflow,
            op_id,
            ctx.index,
            remaining_next,
            collected_next,
            ctx,
        );

        let OpState::Withdrawing(next_state) = next_state else {
            return self.stop_and_exit(Some(&Error::NotWithdrawing));
        };

        let withdrawing = unwrap_or_return!(or_stop::<WithdrawingSpec>(self, op_id));

        let mut withdrawing = withdrawing.replace_state(next_state);

        withdrawing.pay_or_signal_next_withdraw()
    }

    #[private]
    pub fn rebalance_withdraw_01_execute_withdraw_fetch_position(
        &mut self,
        #[callback_result] before_balance: Result<U128, PromiseError>,
        op_id: u64,
        market_id: MarketId,
        batch_limit: Option<u32>,
        before_principal: U128,
    ) -> PromiseOrValue<()> {
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let Ok(before_balance) = before_balance else {
            allocating.market_execution_lock.unlock(market_id);
            let _idle = allocating.into_idle();
            Event::RebalanceWithdrawStopped {
                op_id: op_id.into(),
                market: market_id,
                reason: Some(Reason::Other(Error::BalanceReadFailed.to_string())),
            }
            .emit();
            return PromiseOrValue::Value(());
        };

        Event::VaultBalance {
            amount: before_balance,
        }
        .emit();

        let market_account = allocating.market_account_by_id_or_panic(market_id).clone();

        PromiseOrValue::Promise(
            Self::market_execute_withdraw_and_fetch_position(market_account, batch_limit).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(WITHDRAW_SETTLE_CALLBACK_GAS)
                    .rebalance_withdraw_02_reconcile_position(
                        op_id,
                        market_id,
                        before_principal,
                        before_balance,
                    ),
            ),
        )
    }

    #[private]
    pub fn rebalance_withdraw_02_reconcile_position(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_id: MarketId,
        before_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let reported_principal: u128 = match position {
            Ok(Some(position)) => position.get_deposit().total().into(),
            Ok(None) => 0,
            Err(_) => {
                allocating.market_execution_lock.unlock(market_id);
                let _idle = allocating.into_idle();
                Event::RebalanceWithdrawStopped {
                    op_id: op_id.into(),
                    market: market_id,
                    reason: Some(Reason::Other(Error::PositionReadFailed.to_string())),
                }
                .emit();
                return PromiseOrValue::Value(());
            }
        };

        PromiseOrValue::Promise(
            ext_ft_core::ext(allocating.underlying_asset.contract_id().into())
                .with_static_gas(FT_BALANCE_OF_GAS)
                .ft_balance_of(env::current_account_id())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(WITHDRAW_SETTLE_CALLBACK_GAS)
                        .rebalance_withdraw_03_settle(
                            op_id,
                            market_id,
                            before_principal,
                            U128(reported_principal),
                            before_balance,
                        ),
                ),
        )
    }

    #[private]
    pub fn rebalance_withdraw_03_settle(
        &mut self,
        #[callback_result] after_balance: Result<U128, PromiseError>,
        op_id: u64,
        market_id: MarketId,
        before_principal: U128,
        reported_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let Ok(after_balance) = after_balance else {
            allocating.market_execution_lock.unlock(market_id);
            let _idle = allocating.into_idle();
            Event::RebalanceWithdrawStopped {
                op_id: op_id.into(),
                market: market_id,
                reason: Some(Reason::Other(Error::BalanceReadFailed.to_string())),
            }
            .emit();
            return PromiseOrValue::Value(());
        };

        let _ = allocating.settle_market_principal_and_idle(
            market_id,
            before_principal,
            reported_principal,
            after_balance,
            before_balance,
        );

        allocating.market_execution_lock.unlock(market_id);

        let _idle = allocating.into_idle();
        Event::RebalanceWithdrawCompleted {
            op_id: op_id.into(),
            market: market_id,
        }
        .emit();

        PromiseOrValue::Value(())
    }

    fn payout_resync_and_refund(
        &mut self,
        op_id: u64,
        balance: Result<U128, PromiseError>,
        reason: Option<Reason>,
    ) -> PromiseOrValue<()> {
        let Ok(mut payout) = OpGuard::<PayoutSpec>::expect(self, Some(op_id)) else {
            return PromiseOrValue::Value(());
        };

        let PayoutState {
            op_id,
            receiver,
            amount,
            owner,
            escrow_shares,
            burn_shares: _,
        } = payout.state().clone();

        let (actual_idle, stop_reason) = match balance {
            Ok(U128(v)) => (Some(v), reason),
            Err(_) => (
                None,
                Some(Reason::Other(reason.map_or_else(
                    || Error::BalanceReadFailed.to_string(),
                    |r| format!("{r:?}; {}", Error::BalanceReadFailed),
                ))),
            ),
        };

        Event::PayoutStopped {
            op_id: op_id.into(),
            receiver: receiver.clone(),
            amount: U128(amount),
            reason: stop_reason,
        }
        .emit();

        if let Some(actual_idle) = actual_idle {
            payout.resync_idle_balance(actual_idle);
        }

        if escrow_shares > 0 {
            Gate::bypass_transfer_with(
                &mut payout,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner),
                |e| env::log_str(&e.to_string()),
            );
        }

        payout.pop_head();
        payout.withdraw_route.clear();
        payout.market_execution_lock.clear();
        let _idle = payout.into_idle();

        PromiseOrValue::Value(())
    }

    #[private]
    pub fn stop_and_exit_payout_01_reconcile(
        &mut self,
        #[callback_result] balance: Result<U128, PromiseError>,
        op_id: u64,
        reason: Option<Reason>,
    ) -> PromiseOrValue<()> {
        self.payout_resync_and_refund(op_id, balance, reason)
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
        let Ok(mut payout) = OpGuard::<PayoutSpec>::expect(self, Some(op_id)) else {
            Event::PayoutUnexpectedState {
                op_id: op_id.into(),
                receiver: receiver.clone(),
                amount,
            }
            .emit();
            return;
        };

        let (owner, escrow_shares, expected_amount, burn_shares) = {
            let state = payout.state();
            if state.receiver != receiver {
                Event::PayoutUnexpectedState {
                    op_id: op_id.into(),
                    receiver: receiver.clone(),
                    amount,
                }
                .emit();
                return;
            }

            (
                state.owner.clone(),
                state.escrow_shares,
                state.amount,
                state.burn_shares,
            )
        };

        if result.is_ok() {
            let EscrowSettlement {
                to_burn: burn_shares,
                refund,
            } = EscrowSettlement::new(escrow_shares, burn_shares);

            if burn_shares > 0 {
                // Serious issue: this should be infallible - if the withdrawal panics here we have an escrow settlement error
                let _ = payout
                    .burn(&Nep141Burn::new(burn_shares, env::current_account_id()))
                    .inspect_err(|e| env::log_str(&format!("Failed to burn {e}")));
            }

            if refund > 0 {
                // Note: this should be infallible since we are transferring to an existing owner, and they are unable to unregister from storage
                Gate::bypass_transfer_with(
                    &mut payout,
                    &Nep141Transfer::new(refund, env::current_account_id(), &owner),
                    // Serious issue: this should be infallible - if the transfer panics here we have an escrow settlement error
                    |e| env::log_str(&e.to_string()),
                );
            }
        } else {
            payout.update_idle_balance(IdleBalanceDelta::Increase(expected_amount.into()));
            Gate::bypass_transfer_with(
                &mut payout,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner),
                |e| env::log_str(&e.to_string()),
            );
        }

        payout.pop_head();
        payout.withdraw_route.clear();
        let _idle = payout.into_idle();
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
            _ => panic_with_message("No balance to skim"),
        };
        PromiseOrValue::Promise(
            ext_ft_core::ext(token)
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .with_static_gas(FT_BALANCE_OF_GAS)
                .ft_transfer(recipient, U128(amount), None),
        )
    }

    #[private]
    pub fn refresh_01_settle(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        market_id: MarketId,
        op_id: u64,
        index: u32,
        _before: U128,
    ) -> PromiseOrValue<RealAssetsReport> {
        let Ok(mut refreshing) = OpGuard::<RefreshingSpec>::expect(self, Some(op_id)) else {
            return PromiseOrValue::Value(self.build_real_assets_report());
        };

        if refreshing.state().index != index {
            let idle = refreshing.into_idle();
            return PromiseOrValue::Value(idle.build_real_assets_report());
        }

        if let Ok(Some(position)) = position {
            let total: u128 = position.get_deposit().total().into();
            refreshing.set_market_principal(market_id, total);
        }

        let mut next_state = refreshing.state().clone();
        next_state.index = next_state.index.saturating_add(1);

        if next_state.index as usize >= next_state.plan.len() {
            let report = refreshing.build_real_assets_report();
            Event::RefreshCompleted {
                op_id: op_id.into(),
                markets: next_state.plan,
                total_assets: report.total_assets,
                refreshed_at: report.refreshed_at,
            }
            .emit();
            let _idle = refreshing.into_idle();
            return PromiseOrValue::Value(report);
        }

        let mut refreshing = refreshing.replace_state(next_state);

        refreshing.refresh_step(op_id)
    }
}

impl Contract {
    pub fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &AllocatingState = self.op_state.as_ref();
        let op_id = s.op_id;

        msg.map_or(Event::AllocationCompleted { op_id: s.op_id }, |m| {
            Event::AllocationStopped {
                op_id: s.op_id.into(),
                index: s.index,
                remaining: U128(s.remaining),
                reason: Some(Reason::Other(m.to_string())),
            }
        })
        .emit();

        self.update_idle_balance(IdleBalanceDelta::Increase(s.remaining.into()));

        self.market_execution_lock.clear();

        let allocating = OpGuard::<AllocatingSpec>::expect(self, Some(op_id))
            .unwrap_or_else(|e| panic_with_message(&e.to_string()));
        let _idle = allocating.into_idle();
    }

    pub fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &WithdrawingState = self.op_state.as_ref();
        let op_id = s.op_id;

        Event::WithdrawalStopped {
            op_id: s.op_id.into(),
            index: s.index,
            remaining: U128(s.remaining),
            collected: U128(s.collected),
            reason: msg.map(|m| Reason::Other(m.to_string())),
        }
        .emit();

        self.market_execution_lock.clear();

        let owner = s.owner.clone();

        if s.escrow_shares > 0 {
            Gate::bypass_transfer_with(
                self,
                &Nep141Transfer::new(s.escrow_shares, env::current_account_id(), &owner),
                |e| env::log_str(&e.to_string()),
            );
        }

        self.pop_head();
        self.withdraw_route.clear();

        let withdrawing = OpGuard::<WithdrawingSpec>::expect(self, Some(op_id))
            .unwrap_or_else(|e| panic_with_message(&e.to_string()));
        let _idle = withdrawing.into_idle();
    }

    pub fn stop_and_exit_payout<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s: &PayoutState = self.op_state.as_ref();
        let op_id = s.op_id;
        Event::PayoutStopped {
            op_id: (s.op_id).into(),
            receiver: s.receiver.clone(),
            amount: U128(s.amount),
            reason: msg.map(|m| Reason::Other(m.to_string())),
        }
        .emit();

        let owner = s.owner.clone();
        if s.escrow_shares > 0 {
            Gate::bypass_transfer_with(
                self,
                &Nep141Transfer::new(s.escrow_shares, env::current_account_id(), &owner),
                |e| env::log_str(&e.to_string()),
            );
        }

        self.market_execution_lock.clear();
        self.pop_head();
        self.withdraw_route.clear();

        let payout = OpGuard::<PayoutSpec>::expect(self, Some(op_id))
            .unwrap_or_else(|e| panic_with_message(&e.to_string()));
        let _idle = payout.into_idle();
    }

    pub(crate) fn stop_and_exit<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) -> PromiseOrValue<()> {
        match &self.op_state {
            OpState::Allocating(_) => self.stop_and_exit_allocating(msg),
            OpState::Withdrawing(_) => self.stop_and_exit_withdrawing(msg),
            OpState::Refreshing(_) => {
                let refreshing = OpGuard::<RefreshingSpec>::expect(self, None)
                    .unwrap_or_else(|e| panic_with_message(&e.to_string()));
                let _idle = refreshing.into_idle();
                Event::OperationStoppedWhileIdle {
                    reason: msg.map(|m| Reason::Other(m.to_string())),
                }
                .emit();
            }
            OpState::Payout(s) => {
                let op_id = s.op_id;
                let reason = msg.map(|m| Reason::Other(m.to_string()));
                return PromiseOrValue::Promise(
                    ext_ft_core::ext(self.underlying_asset.contract_id().into())
                        .with_static_gas(FT_BALANCE_OF_GAS)
                        .ft_balance_of(env::current_account_id())
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(AFTER_SEND_TO_USER_GAS)
                                .stop_and_exit_payout_01_reconcile(op_id, reason),
                        ),
                );
            }
            OpState::Idle => {
                Event::OperationStoppedWhileIdle {
                    reason: msg.map(|m| Reason::Other(m.to_string())),
                }
                .emit();
            }
        }
        PromiseOrValue::Value(())
    }

    #[allow(dead_code)]
    /// Validate current op is Allocating and return the state reference.
    pub(crate) fn ctx_allocating(&self, op_id: u64) -> Result<&AllocatingState, Error> {
        AllocatingSpec::validate(self, Some(op_id))
    }

    /// Validate current op is Withdrawing and return context tuple
    ///
    /// # Errors
    /// Returns an error if the operation is not currently withdrawing.
    pub fn ctx_withdrawing(&self, op_id: u64) -> Result<&WithdrawingState, Error> {
        WithdrawingSpec::validate(self, Some(op_id))
    }

    /// Combined helper for withdrawing callbacks: validate ctx and resolve market.
    /// Returns (cloned context, market id) on success, or calls `stop_and_exit` and returns Err on failure.
    pub(crate) fn withdraw_ctx_and_market_or_exit(
        &mut self,
        op_id: u64,
        market: MarketId,
    ) -> Result<WithdrawingState, PromiseOrValue<()>> {
        let ctx = match WithdrawingSpec::validate(self, Some(op_id)) {
            Ok(ctx) => ctx.clone(),
            Err(e) => return Err(self.stop_and_exit(Some(&e))),
        };

        let Some(expected_market) = self.withdraw_route.get(ctx.index as usize).copied() else {
            return Err(self.stop_and_exit(Some(&Error::MissingMarket(market))));
        };

        if expected_market != market {
            return Err(self.stop_and_exit(Some(&Error::MarketDrifted {
                expected: expected_market,
                actual: market,
            })));
        }

        if self.market_account_by_id(market).is_none() {
            return Err(self.stop_and_exit(Some(&Error::MissingMarket(market))));
        }

        Ok(ctx)
    }

    #[must_use]
    pub fn compute_withdraw_deltas(
        before_principal: U128,
        new_principal_reported: U128,
        after_balance: U128,
        before_balance: U128,
    ) -> (u128, u128, u128) {
        // Principal drop as reported by the market
        let principal_delta = before_principal.0.saturating_sub(new_principal_reported.0);

        let inflow = after_balance.0.saturating_sub(before_balance.0);

        // Compute effective principal drop we can book (conservative on shortfall)
        let creditable = principal_delta.min(inflow);
        (principal_delta, inflow, creditable)
    }

    pub fn update_idle_balance(&mut self, delta: IdleBalanceDelta) {
        let idle_balance = self.idle_balance;
        self.idle_balance = delta.apply(idle_balance);
    }

    fn resync_idle_balance(&mut self, actual: u128) {
        match actual.cmp(&self.idle_balance) {
            Ordering::Greater => self.update_idle_balance(IdleBalanceDelta::Increase(U128(
                actual.saturating_sub(self.idle_balance),
            ))),
            Ordering::Less => self.update_idle_balance(IdleBalanceDelta::Decrease(U128(
                self.idle_balance.saturating_sub(actual),
            ))),
            Ordering::Equal => {}
        }
    }

    fn market_execute_withdraw_and_fetch_position(
        market: AccountId,
        batch_limit: Option<u32>,
    ) -> Promise {
        ext_market::ext(market.clone())
            // NOTE: gas might be incorrect here
            .with_static_gas(Gas::from_tgas(
                EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS.as_tgas()
                    * u64::from(batch_limit.unwrap_or(1)),
            ))
            .with_unused_gas_weight(0)
            .execute_next_supply_withdrawal_request(batch_limit)
            .then(
                ext_market::ext(market)
                    .with_static_gas(GET_SUPPLY_POSITION_GAS)
                    .with_unused_gas_weight(0)
                    .get_supply_position(env::current_account_id()),
            )
    }

    fn settle_market_principal_and_idle(
        &mut self,
        market_id: MarketId,
        before_principal: U128,
        reported_principal: U128,
        after_balance: U128,
        before_balance: U128,
    ) -> (u128, u128, u128) {
        let (principal_delta, inflow, creditable) = Self::compute_withdraw_deltas(
            before_principal,
            reported_principal,
            after_balance,
            before_balance,
        );

        let effective_principal = before_principal.0.saturating_sub(creditable);

        self.set_market_principal(market_id, effective_principal);

        self.resync_idle_balance(after_balance.0);

        (principal_delta, inflow, creditable)
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

    let (payout_delta, remaining_next, collected_next) =
        determine_payout_delta(remaining_total, collected_total, withdrawn);

    WithdrawReconciliation {
        payout_delta,
        remaining_next,
        collected_next,
        idle_delta,
    }
}

pub fn determine_payout_delta(
    remaining_total: u128,
    collected_total: u128,
    withdrawn: u128,
) -> (u128, u128, u128) {
    let payout_delta = withdrawn.min(remaining_total);
    let remaining_next = remaining_total.saturating_sub(payout_delta);
    let collected_next = collected_total.saturating_add(payout_delta);
    (payout_delta, remaining_next, collected_next)
}

pub fn state_on_delta_inflow(
    principal_delta: u128,
    inflow: u128,
    op_id: u64,
    route_index: u32,
    remaining_next: u128,
    collected_next: u128,
    ctx: WithdrawingState,
) -> OpState {
    let state = match principal_delta.cmp(&inflow) {
        Ordering::Less | Ordering::Equal if principal_delta > 0 => WithdrawingState {
            op_id,
            index: route_index.saturating_add(1),
            remaining: remaining_next,
            receiver: ctx.receiver,
            collected: collected_next,
            owner: ctx.owner,
            escrow_shares: ctx.escrow_shares,
        },
        _ => WithdrawingState {
            op_id,
            index: route_index,
            remaining: remaining_next,
            receiver: ctx.receiver,
            collected: collected_next,
            owner: ctx.owner,
            escrow_shares: ctx.escrow_shares,
        },
    };
    OpState::Withdrawing(state)
}
