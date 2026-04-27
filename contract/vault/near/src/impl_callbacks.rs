#![allow(clippy::too_many_arguments)]

use core::cmp::Ordering;
use std::fmt::Display;

use crate::{
    governance::Gate,
    near,
    op_guard::{AllocatingSpec, OpGuard, PayoutSpec, RefreshingSpec, WithdrawingSpec},
    Contract, ContractExt, Error, Nep141Controller, RealAssetsReport,
};

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env,
    json_types::{U128, U64},
    AccountId, Gas, NearToken, Promise, PromiseError, PromiseOrValue,
};
use near_sdk_contract_tools::ft::{Nep141Burn, Nep141Transfer};

use crate::policy::FencingToken;
use templar_common::{
    guard::GuardSpec,
    market::ext_market,
    panic_with_message,
    supply::SupplyPosition,
    vault::{
        AllocatingState, AllocationPositionIssueKind, Event, IdleBalanceDelta, MarketId, OpState,
        PayoutState, PositionReportOutcome, Reason, WithdrawalAccountingKind, WithdrawingState,
        AFTER_SEND_TO_USER_GAS, EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS, FT_BALANCE_OF_GAS,
        GET_SUPPLY_POSITION_GAS, SUPPLY_POSITION_READ_CALLBACK_GAS, WITHDRAW_SETTLE_CALLBACK_GAS,
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

pub(crate) fn or_stop<S>(
    contract: &mut Contract,
    op_id: u64,
) -> Result<OpGuard<'_, S>, PromiseOrValue<()>>
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

fn is_stale_market_callback(
    contract: &Contract,
    market: MarketId,
    op_id: u64,
    fencing_token: U64,
) -> bool {
    !contract
        .market_execution_lock
        .has_current_lease(market, op_id, FencingToken(fencing_token.0))
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
                current_index.into(),
                market_index.into(),
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

        allocating.set_market_principal(market_id, new_principal);
        let _ = allocating;
        let kernel_state = self.op_state.clone();
        let result = templar_vault_kernel::transitions::allocation_step_callback(
            kernel_state,
            true,
            accepted_event,
            op_id,
        )
        .unwrap_or_else(|_| panic_with_message("Kernel allocation step failed"));
        self.apply_kernel_op_state(&result.new_state);

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
        market: MarketId,
        need: U128,
    ) -> PromiseOrValue<()> {
        let _ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        if did_create.is_ok() {
            self.market_execution_lock.lock(
                market,
                op_id,
                u64::MAX.saturating_sub(env::block_timestamp()),
            );
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
        fencing_token: U64,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
        let _ctx = unwrap_or_return!(self.withdraw_ctx_and_market_or_exit(op_id, market));

        let Ok(before_balance) = before_balance else {
            return self.stop_and_exit(Some(&Error::BalanceReadFailed));
        };

        let principal = self.principal_of(market);

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
                        fencing_token,
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
        fencing_token: U64,
        principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
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
                // Treat missing position as zero principal and continue to balance settlement.
                // This is intentionally different from allocation (which stops on missing).
                // In withdrawal, a missing position means the market has no funds for us.
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
                            fencing_token,
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
        fencing_token: U64,
        before_principal: U128,
        reported_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
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

        self.market_execution_lock
            .unlock(market, op_id, FencingToken(fencing_token.0));

        // Reconcile remaining/collected based on credited inflow only
        let WithdrawReconciliation {
            payout_delta: principal_payout,
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
        let (extra_payout, remaining_next, collected_next) =
            determine_payout_delta(remaining_next, collected_next, extra);

        let payout_delta = principal_payout.saturating_add(extra_payout);

        let desired_index = match principal_delta.cmp(&inflow) {
            Ordering::Less | Ordering::Equal if principal_delta > 0 => ctx.index.saturating_add(1),
            _ => ctx.index,
        };

        let kernel_state = self.op_state.clone();
        let result = templar_vault_kernel::transitions::withdrawal_step_callback(
            kernel_state,
            op_id,
            payout_delta,
        )
        .unwrap_or_else(|_| panic_with_message("Kernel withdrawal step failed"));
        self.apply_kernel_op_state(&result.new_state);
        if let OpState::Withdrawing(state) = &mut self.op_state {
            state.index = desired_index;
        }

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
                    let owner = withdrawing.resolve_account(&ctx.owner);
                    withdrawing
                        .transfer(&Nep141Transfer::new(ctx.escrow_shares, &self_id, &owner))
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

        self.pay_or_signal_next_withdraw()
    }

    #[private]
    pub fn rebalance_withdraw_01_execute_withdraw_fetch_position(
        &mut self,
        #[callback_result] before_balance: Result<U128, PromiseError>,
        op_id: u64,
        market_id: MarketId,
        fencing_token: U64,
        batch_limit: Option<u32>,
        before_principal: U128,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market_id, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let Ok(before_balance) = before_balance else {
            allocating.market_execution_lock.unlock(
                market_id,
                op_id,
                FencingToken(fencing_token.0),
            );
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
                        fencing_token,
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
        fencing_token: U64,
        before_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market_id, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let reported_principal: u128 = match position {
            Ok(Some(position)) => position.get_deposit().total().into(),
            // Treat missing position as zero - market has no funds for us
            Ok(None) => 0,
            Err(_) => {
                allocating.market_execution_lock.unlock(
                    market_id,
                    op_id,
                    FencingToken(fencing_token.0),
                );
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
                            fencing_token,
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
        fencing_token: U64,
        before_principal: U128,
        reported_principal: U128,
        before_balance: U128,
    ) -> PromiseOrValue<()> {
        if is_stale_market_callback(self, market_id, op_id, fencing_token) {
            return PromiseOrValue::Value(());
        }
        let mut allocating = unwrap_or_return!(or_stop::<AllocatingSpec>(self, op_id));

        let Ok(after_balance) = after_balance else {
            allocating.market_execution_lock.unlock(
                market_id,
                op_id,
                FencingToken(fencing_token.0),
            );
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

        allocating
            .market_execution_lock
            .unlock(market_id, op_id, FencingToken(fencing_token.0));

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
            request_id: _,
            receiver,
            amount,
            owner,
            escrow_shares,
            burn_shares: _,
        } = payout.state().clone();
        let receiver_account = payout.resolve_account(&receiver);
        let owner_account = payout.resolve_account(&owner);

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
            receiver: receiver_account,
            amount: U128(amount),
            reason: stop_reason,
        }
        .emit();

        if let Some(actual_idle) = actual_idle {
            payout.resync_idle_balance_to(actual_idle);
        }

        if escrow_shares > 0 {
            // Must be infallible - panic to prevent orphaned shares in escrow.
            Gate::bypass_transfer_with(
                &mut payout,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner_account),
                |e| {
                    templar_common::panic_with_message(&format!(
                        "Payout stop escrow refund failed: {e}"
                    ))
                },
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
            let expected_receiver = payout.resolve_account(&state.receiver);
            if expected_receiver != receiver {
                Event::PayoutUnexpectedState {
                    op_id: op_id.into(),
                    receiver: receiver.clone(),
                    amount,
                }
                .emit();
                return;
            }

            (
                payout.resolve_account(&state.owner),
                state.escrow_shares,
                state.amount,
                state.burn_shares,
            )
        };

        if result.is_ok() {
            let refund = escrow_shares.saturating_sub(burn_shares);

            if burn_shares > 0 {
                // This must be infallible - panic to prevent orphaned shares in escrow.
                payout
                    .burn(&Nep141Burn::new(burn_shares, env::current_account_id()))
                    .unwrap_or_else(|e| {
                        templar_common::panic_with_message(&format!(
                            "Escrow settlement burn failed: {e}"
                        ))
                    });
            }

            if refund > 0 {
                // This must be infallible - panic to prevent orphaned shares in escrow.
                Gate::bypass_transfer_with(
                    &mut payout,
                    &Nep141Transfer::new(refund, env::current_account_id(), &owner),
                    |e| {
                        templar_common::panic_with_message(&format!(
                            "Escrow settlement refund failed: {e}"
                        ))
                    },
                );
            }
        } else {
            payout.update_idle_balance(IdleBalanceDelta::Increase(expected_amount.into()));
            // On payout failure, refund all escrow shares. Must be infallible.
            Gate::bypass_transfer_with(
                &mut payout,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner),
                |e| {
                    templar_common::panic_with_message(&format!(
                        "Escrow settlement failure refund failed: {e}"
                    ))
                },
            );
        }

        payout.pop_head();
        payout.withdraw_route.clear();
        payout.market_execution_lock.clear();
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
        before: U128,
    ) -> PromiseOrValue<RealAssetsReport> {
        let _ = before;
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

        let _ = refreshing;

        let kernel_state = self.op_state.clone();
        let result = templar_vault_kernel::transitions::refresh_step_callback(kernel_state, op_id)
            .unwrap_or_else(|_| panic_with_message("Kernel refresh step failed"));
        self.apply_kernel_op_state(&result.new_state);

        let (next_index, next_plan) = match &self.op_state {
            OpState::Refreshing(state) => (state.index, state.plan.clone()),
            _ => return PromiseOrValue::Value(self.build_real_assets_report()),
        };

        if next_index as usize >= next_plan.len() {
            let report = self.build_real_assets_report();
            self.last_refresh_ns = u64::from(report.refreshed_at);
            Event::RefreshCompleted {
                op_id: op_id.into(),
                markets: next_plan.into_iter().map(MarketId::from).collect(),
                total_assets: report.total_assets,
                refreshed_at: report.refreshed_at,
            }
            .emit();

            let kernel_state = self.op_state.clone();
            let result = templar_vault_kernel::transitions::complete_refresh(kernel_state, op_id)
                .unwrap_or_else(|_| panic_with_message("Kernel complete refresh failed"));
            self.apply_kernel_op_state(&result.new_state);
            return PromiseOrValue::Value(report);
        }

        self.refresh_step(op_id)
    }

    #[private]
    #[allow(
        clippy::too_many_lines,
        reason = "callback reconciles several terminal and recovery branches for one async operation"
    )]
    pub fn resync_idle_balance_01_settle(
        &mut self,
        #[callback_result] balance: Result<U128, PromiseError>,
        op_id: u64,
        caller: AccountId,
        before_idle: U128,
        started_at_ns: u64,
    ) -> templar_common::vault::ResyncIdleReport {
        use templar_common::vault::{IdleResyncOutcome, ResyncIdleReport};

        let finished_at_ns = env::block_timestamp();
        let inflight = self.idle_resync_inflight_op_id;

        if inflight == 0 || inflight != op_id {
            Event::IdleResyncCallbackIgnored {
                op_id: op_id.into(),
                reason: Reason::Other("mismatched op_id".to_string()),
            }
            .emit();
            return ResyncIdleReport {
                outcome: IdleResyncOutcome::Ignored,
                before_idle,
                actual_idle: before_idle,
                after_idle: before_idle,
                increased_by: U128(0),
                decreased_by: U128(0),
                fee_anchor_bump: U128(0),
                resynced_at_ns: finished_at_ns.into(),
            };
        }

        let valid_state = matches!(
            self.op_state,
            OpState::Allocating(AllocatingState {
                op_id: cur,
                index: 0,
                remaining: 0,
                ref plan
            }) if cur == op_id && plan.is_empty()
        );

        if !valid_state {
            Event::IdleResyncStopped {
                op_id: op_id.into(),
                caller,
                before_idle,
                reason: Some(Reason::Other("IdleResyncUnexpectedState".to_string())),
                finished_at_ns: finished_at_ns.into(),
            }
            .emit();
            self.idle_resync_inflight_op_id = 0;
            self.set_op_state(OpState::Idle);
            return ResyncIdleReport {
                outcome: IdleResyncOutcome::UnexpectedState,
                before_idle,
                actual_idle: before_idle,
                after_idle: before_idle,
                increased_by: U128(0),
                decreased_by: U128(0),
                fee_anchor_bump: U128(0),
                resynced_at_ns: finished_at_ns.into(),
            };
        }

        let Ok(U128(actual_idle)) = balance else {
            Event::IdleResyncStopped {
                op_id: op_id.into(),
                caller,
                before_idle,
                reason: Some(Reason::Other(Error::BalanceReadFailed.to_string())),
                finished_at_ns: finished_at_ns.into(),
            }
            .emit();
            self.idle_resync_inflight_op_id = 0;
            self.set_op_state(OpState::Idle);
            return ResyncIdleReport {
                outcome: IdleResyncOutcome::BalanceReadFailed,
                before_idle,
                actual_idle: before_idle,
                after_idle: before_idle,
                increased_by: U128(0),
                decreased_by: U128(0),
                fee_anchor_bump: U128(0),
                resynced_at_ns: finished_at_ns.into(),
            };
        };

        self.resync_idle_balance_to(actual_idle);
        let after_idle = self.idle_balance;

        let increased_by = after_idle.saturating_sub(before_idle.0);
        let decreased_by = before_idle.0.saturating_sub(after_idle);

        self.fee_anchor.total_assets =
            U128(self.fee_anchor.total_assets.0.saturating_add(increased_by));

        Event::IdleResyncCompleted {
            op_id: op_id.into(),
            caller,
            before_idle,
            actual_idle: U128(actual_idle),
            after_idle: U128(after_idle),
            increased_by: U128(increased_by),
            decreased_by: U128(decreased_by),
            fee_anchor_bump: U128(increased_by),
            finished_at_ns: finished_at_ns.into(),
        }
        .emit();

        self.idle_resync_inflight_op_id = 0;
        self.set_op_state(OpState::Idle);

        ResyncIdleReport {
            outcome: IdleResyncOutcome::Ok,
            before_idle,
            actual_idle: U128(actual_idle),
            after_idle: U128(after_idle),
            increased_by: U128(increased_by),
            decreased_by: U128(decreased_by),
            fee_anchor_bump: U128(increased_by),
            resynced_at_ns: started_at_ns.max(finished_at_ns).into(),
        }
    }
}

impl Contract {
    pub fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s = self
            .op_state
            .as_allocating()
            .unwrap_or_else(|| panic_with_message("OpState::Allocating expected"));

        msg.map_or(
            Event::AllocationCompleted {
                op_id: s.op_id.into(),
            },
            |m| Event::AllocationStopped {
                op_id: s.op_id.into(),
                index: s.index,
                remaining: U128(s.remaining),
                reason: Some(Reason::Other(m.to_string())),
            },
        )
        .emit();

        self.update_idle_balance(IdleBalanceDelta::Increase(s.remaining.into()));

        self.market_execution_lock.clear();

        // Clear idle resync flag in case this was a stuck resync_idle_balance operation.
        // The flag blocks withdraw/redeem, so it must be cleared on any allocation abort.
        self.idle_resync_inflight_op_id = 0;

        self.set_op_state(OpState::Idle);
    }

    pub fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s = self
            .op_state
            .as_withdrawing()
            .unwrap_or_else(|| panic_with_message("OpState::Withdrawing expected"));

        Event::WithdrawalStopped {
            op_id: s.op_id.into(),
            index: s.index,
            remaining: U128(s.remaining),
            collected: U128(s.collected),
            reason: msg.map(|m| Reason::Other(m.to_string())),
        }
        .emit();

        let owner = self.resolve_account(&s.owner);
        let escrow_shares = s.escrow_shares;

        self.refund_escrow_and_go_idle(owner, escrow_shares, "Withdrawing");
    }

    pub fn stop_and_exit_payout<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        let s = self
            .op_state
            .as_payout()
            .unwrap_or_else(|| panic_with_message("OpState::Payout expected"));

        Event::PayoutStopped {
            op_id: (s.op_id).into(),
            receiver: self.resolve_account(&s.receiver),
            amount: U128(s.amount),
            reason: msg.map(|m| Reason::Other(m.to_string())),
        }
        .emit();

        let owner = self.resolve_account(&s.owner);
        let escrow_shares = s.escrow_shares;

        self.refund_escrow_and_go_idle(owner, escrow_shares, "Payout");
    }

    /// Shared cleanup for Withdrawing and Payout stop-and-exit paths:
    /// refund escrowed shares, clear locks/queue, and transition to Idle.
    fn refund_escrow_and_go_idle(&mut self, owner: AccountId, escrow_shares: u128, context: &str) {
        self.market_execution_lock.clear();

        if escrow_shares > 0 {
            // Must be infallible - panic to prevent orphaned shares in escrow.
            Gate::bypass_transfer_with(
                self,
                &Nep141Transfer::new(escrow_shares, env::current_account_id(), &owner),
                |e| {
                    templar_common::panic_with_message(&format!(
                        "{context} stop escrow refund failed: {e}"
                    ))
                },
            );
        }

        self.pop_head();
        self.withdraw_route.clear();
        self.set_op_state(OpState::Idle);
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

    fn resync_idle_balance_to(&mut self, actual: u128) {
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
            .with_static_gas(Gas::from_tgas(
                EXECUTE_NEXT_SUPPLY_WITHDRAW_REQ_GAS
                    .as_tgas()
                    .saturating_mul(u64::from(batch_limit.unwrap_or(1))),
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

        self.resync_idle_balance_to(after_balance.0);

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
