use std::fmt::Display;

use crate::{
    ext_self, near, Contract, ContractExt, Error, Nep141Controller, OpState, GAS_CB, GAS_XFER,
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env, json_types::U128, serde_json, AccountId, Gas, NearToken, Promise, PromiseError,
    PromiseOrValue,
};
use near_sdk_contract_tools::ft::nep141::GAS_FOR_FT_TRANSFER_CALL;
use templar_common::{market::ext_market, supply::SupplyPosition};

#[near]
impl Contract {
    const AFTER_SUPPLY_ENSURE_GAS: Gas = Gas::from_tgas(20);
    const GET_SUPPLY_POSITION_GAS: Gas = Gas::from_tgas(20);

    #[private]
    pub fn after_supply_1_check(
        &mut self,
        #[callback_result] supply_refund: Result<U128, PromiseError>,
        op_id: u64,
        market_index: u32,
        attempted: U128,
    ) -> PromiseOrValue<()> {
        // Invariant: Index drift or stale op_id results in a graceful stop
        match &self.op_state {
            OpState::Allocating { op_id: cur, .. } if *cur == op_id => {}
            _ => return self.stop_and_exit(Some(&Error::NotAllocating(self.op_state.clone()))),
        }

        let Some(market) = self.supply_queue.get(market_index) else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        // If the transfer failed, do not attempt to reconcile; stop and leave remaining untouched
        if supply_refund.is_err() {
            env::log_str(&format!(
                "after_supply_1_check: transfer failed; stopping (op_id={}, market={}, index={}, attempted={})",
                op_id, market, market_index, attempted.0
            ));
            return self.stop_and_exit(Some(&Error::MarketTransferFailed));
        }

        let before = self.market_supply.get(market).unwrap_or(&0);

        let fetch_pos = ext_market::ext(market.clone())
            .with_static_gas(Self::GET_SUPPLY_POSITION_GAS)
            .get_supply_position(env::current_account_id());

        PromiseOrValue::Promise(
            fetch_pos.then(
                ext_self::ext(env::current_account_id())
                    .with_static_gas(GAS_CB)
                    .after_supply_2_read(
                        op_id,
                        market_index,
                        U128(*before),
                        attempted,
                        supply_refund.unwrap_or(U128(0)),
                    ),
            ),
        )
    }

    // FIXME: no panics in this function! This will cause to spin if the op changes
    #[private]
    pub fn after_supply_2_read(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_index: u32,
        before: U128,
        attempted: U128,
        refunded: U128,
    ) -> PromiseOrValue<()> {
        let (idx, rem) = match &self.op_state {
            OpState::Allocating {
                op_id: cur,
                index,
                remaining,
            } if *cur == op_id => (*index, *remaining),
            _ => return self.stop_and_exit(Some(&Error::NotAllocating(self.op_state.clone()))),
        };

        // Invariant: Index drift or stale op_id results in a graceful stop
        if idx != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(idx, market_index)));
        }

        let Some(market) = self.supply_queue.get(market_index) else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        let (new_principal, remaining_next) = match position {
            Ok(Some(position)) => {
                let new_principal: u128 = position.get_deposit().total().into();
                let accepted = new_principal.saturating_sub(before.0);
                let remaining = rem.saturating_sub(accepted);
                (new_principal, remaining)
            }
            Ok(None) => {
                env::log_str(&format!(
                    "after_supply_2_read: position None; stopping (op_id={}, market={}, index={}, attempted={}, refunded={})",
                    op_id, market, market_index, attempted.0, refunded.0
                ));
                return self.stop_and_exit(Some(&Error::MissingSupplyPosition));
            }
            Err(_) => {
                env::log_str(&format!(
                    "after_supply_2_read: position read failed; stopping (op_id={}, market={}, index={}, attempted={}, refunded={})",
                    op_id, market, market_index, attempted.0, refunded.0
                ));
                return self.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        self.market_supply.insert(market.clone(), new_principal);
        // Invariant: withdraw_queue gains any market with new_principal > 0
        if new_principal > 0 && !self.withdraw_queue.iter().any(|m| m == market) {
            self.withdraw_queue.push(market.clone());
        }

        self.op_state = OpState::Allocating {
            op_id,
            index: market_index + 1,
            remaining: remaining_next,
        };
        self.step_allocation();
        PromiseOrValue::Value(())
    }

    #[private]
    pub fn after_create_withdraw_req(
        &mut self,
        #[callback_result] did_create: Result<(), PromiseError>,
        op_id: u64,
        market_index: u32,
        need: U128,
    ) -> PromiseOrValue<()> {
        let (idx, rem, recv, coll, owner, escrow_shares) = match &self.op_state {
            OpState::Withdrawing {
                op_id: cur,
                index,
                remaining,
                receiver,
                collected,
                owner,
                escrow_shares,
            } if *cur == op_id => (
                *index,
                *remaining,
                receiver.clone(),
                *collected,
                owner.clone(),
                *escrow_shares,
            ),
            _ => return self.stop_and_exit(Some(&Error::NotWithdrawing(self.op_state.clone()))),
        };

        // Invariant: Index drift or stale op_id results in a graceful stop
        if idx != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(idx, market_index)));
        }

        let Some(market) = self.withdraw_queue.get(market_index) else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        if let Ok(()) = did_create {
            PromiseOrValue::Promise(
                ext_market::ext(market.clone())
                    .with_static_gas(GAS_XFER)
                    .execute_next_supply_withdrawal_request()
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(GAS_CB)
                            .after_exec_withdraw_req(op_id, market_index, need),
                    ),
            )
        } else {
            env::log_str("create_supply_withdrawal_request failed; moving to next market");
            self.op_state = OpState::Withdrawing {
                op_id,
                index: market_index + 1,
                remaining: rem,
                receiver: recv,
                collected: coll,
                owner,
                escrow_shares,
            };
            self.step_withdraw()
        }
    }

    #[private]
    pub fn after_exec_withdraw_req(
        &mut self,
        op_id: u64,
        market_index: u32,
        need: U128,
    ) -> PromiseOrValue<()> {
        let (idx, _rem, _recv, _coll, _owner, _escrow_shares) = match &self.op_state {
            OpState::Withdrawing {
                op_id: cur,
                index,
                remaining,
                receiver,
                collected,
                owner,
                escrow_shares,
            } if *cur == op_id => (
                *index,
                *remaining,
                receiver.clone(),
                *collected,
                owner.clone(),
                *escrow_shares,
            ),
            _ => return self.stop_and_exit(Some(&Error::NotWithdrawing(self.op_state.clone()))),
        };

        // Invariant: Index drift or stale op_id results in a graceful stop
        if idx != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(idx, market_index)));
        }

        let Some(market) = self.withdraw_queue.get(market_index) else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        // Verify actual withdrawal by reading market position after execution
        let before = *self.market_supply.get(market).unwrap_or(&0);
        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(Self::GET_SUPPLY_POSITION_GAS)
                .get_supply_position(env::current_account_id())
                .then(
                    Promise::new(env::current_account_id()).function_call(
                        "after_exec_withdraw_read".to_string(),
                        serde_json::to_vec(&serde_json::json!({
                            "op_id": op_id,
                            "market_index": market_index,
                            "before": U128(before),
                            "need": need,
                        }))
                        .expect("json"),
                        NearToken::from_yoctonear(0),
                        GAS_CB,
                    ),
                ),
        )
    }

    #[private]
    pub fn after_exec_withdraw_read(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_index: u32,
        before: U128,
        need: U128,
    ) -> PromiseOrValue<()> {
        let (idx, rem, recv, coll, owner, escrow_shares) = match &self.op_state {
            OpState::Withdrawing {
                op_id: cur,
                index,
                remaining,
                receiver,
                collected,
                owner,
                escrow_shares,
            } if *cur == op_id => (
                *index,
                *remaining,
                receiver.clone(),
                *collected,
                owner.clone(),
                *escrow_shares,
            ),
            _ => return self.stop_and_exit(Some(&Error::NotWithdrawing(self.op_state.clone()))),
        };

        if idx != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(idx, market_index)));
        }

        let Some(market) = self.withdraw_queue.get(market_index) else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        let before_principal = before.0;
        let new_principal = match position {
            Ok(Some(position)) => {
                let np: u128 = position.get_deposit().total().into();
                np
            }
            Ok(None) => {
                // No position => treat as principal = 0
                env::log_str(&format!(
                    "after_exec_withdraw_read: no position; treating principal as 0 (op_id={}, market={}, index={}, before={}, need={})",
                    op_id, market, market_index, before_principal, need.0
                ));
                0
            }
            Err(_) => {
                env::log_str(&format!(
                    "after_exec_withdraw_read: get_supply_position failed; op_id={}, market={}, index={}, assuming no change (before={}, need={})",
                    op_id, market, market_index, before_principal, need.0
                ));
                before_principal
            }
        };

        let withdrawn = before_principal.saturating_sub(new_principal);
        let credited = withdrawn.min(need.0);

        // Update accounting to match market state
        self.market_supply.insert(market.clone(), new_principal);
        let remaining = rem.saturating_sub(credited);
        let collected = coll.saturating_add(credited);
        if credited > 0 {
            self.idle_balance = self.idle_balance.saturating_add(credited);
        }

        if remaining == 0 {
            if collected > 0 {
                self.op_state = OpState::Payout {
                    op_id,
                    receiver: recv.clone(),
                    amount: collected,
                    owner: owner.clone(),
                    escrow_shares,
                };
                PromiseOrValue::Promise(
                    self.underlying_asset
                        .clone()
                        .transfer(recv.clone(), U128(collected).into())
                        .then(
                            ext_self::ext(env::current_account_id())
                                .with_static_gas(GAS_CB)
                                .after_send_to_user(op_id, recv, U128(collected)),
                        ),
                )
            } else {
                // Nothing collected; refund escrowed shares
                let self_id = env::current_account_id();
                self.withdraw_unchecked(&self_id, escrow_shares)
                    .expect("Failed to release escrowed shares");
                self.deposit_unchecked(&owner, escrow_shares);
                self.op_state = OpState::Idle;
                PromiseOrValue::Value(())
            }
        } else {
            self.op_state = OpState::Withdrawing {
                op_id,
                index: market_index + 1,
                remaining,
                receiver: recv,
                collected,
                owner,
                escrow_shares,
            };
            self.step_withdraw()
        }
    }

    #[private]
    pub fn after_send_to_user(
        &mut self,
        #[callback_result] result: Result<(), PromiseError>,
        op_id: u64,
        receiver: AccountId,
        amount: U128,
    ) -> bool {
        let (owner, escrow_shares, payout_amount) = match &self.op_state {
            OpState::Payout {
                op_id: cur,
                receiver: r,
                amount: a,
                owner,
                escrow_shares,
            } if *cur == op_id && *r == receiver => (owner.clone(), *escrow_shares, *a),
            _ => {
                env::log_str("after_send_to_user: unexpected op_state; ignoring");
                return false;
            }
        };

        if let Ok(()) = result {
            // Invariant: On payout success, idle_balance -= payout_amount and escrowed shares are burned
            self.idle_balance = self.idle_balance.saturating_sub(payout_amount);
            self.withdraw_unchecked(&env::current_account_id(), escrow_shares)
                .expect("Failed to burn escrowed shares");
            self.op_state = OpState::Idle;
            true
        } else {
            // Invariant: On payout failure, refund escrow to owner and leave idle_balance unchanged
            #[allow(clippy::expect_used, reason = "No side effects")]
            self.transfer_unchecked(&env::current_account_id(), &owner, escrow_shares)
                .expect("Failed to release escrowed shares");
            self.op_state = OpState::Idle;
            false
        }
    }

    fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        if let Some(msg) = msg {
            env::log_str(format!("Allocation stopped: {msg}").as_str());
        }
        if let OpState::Allocating { remaining, .. } = &self.op_state {
            if *remaining > 0 {
                self.idle_balance = self.idle_balance.saturating_add(*remaining);
            }
        }
        self.plan = None;
        self.op_state = OpState::Idle;
    }

    /// Stop helper for Withdrawing: refund escrowed shares to owner and go Idle.
    fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        if let Some(msg) = msg {
            env::log_str(format!("Withdrawal stopped: {msg}").as_str());
        }
        // Take copies to avoid holding immutable borrows across mutable self calls.
        let (owner_acc, escrow) = match &self.op_state {
            OpState::Withdrawing {
                owner,
                escrow_shares,
                ..
            } => (Some(owner.clone()), *escrow_shares),
            _ => (None, 0),
        };
        if let (Some(owner_acc), escrow) = (owner_acc, escrow) {
            if escrow > 0 {
                let self_id = env::current_account_id();
                #[allow(clippy::expect_used, reason = "No side effects")]
                self.transfer_unchecked(&self_id, &owner_acc, escrow)
                    .expect("Failed to release escrowed shares");
            }
        }
        self.op_state = OpState::Idle;
    }

    /// Payout: refund escrowed shares to owner and go Idle.
    fn stop_and_exit_payout<T: Display + core::fmt::Debug + ?Sized>(&mut self, msg: Option<&T>) {
        if let Some(msg) = msg {
            env::log_str(format!("Payout stopped: {msg}").as_str());
        }
        // Take copies to avoid holding immutable borrows across mutable self calls.
        let (owner_acc, escrow) = match &self.op_state {
            OpState::Payout {
                owner,
                escrow_shares,
                ..
            } => (Some(owner.clone()), *escrow_shares),
            _ => (None, 0),
        };
        if let (Some(owner_acc), escrow) = (owner_acc, escrow) {
            if escrow > 0 {
                let self_id = env::current_account_id();
                #[allow(clippy::expect_used, reason = "No side effects")]
                self.transfer_unchecked(&self_id, &owner_acc, escrow)
                    .expect("Failed to release escrowed shares");
            }
        }
        self.op_state = OpState::Idle;
    }

    pub(crate) fn stop_and_exit<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) -> PromiseOrValue<()> {
        match self.op_state {
            OpState::Allocating { .. } => self.stop_and_exit_allocating(msg),
            OpState::Withdrawing { .. } => self.stop_and_exit_withdrawing(msg),
            OpState::Payout { .. } => self.stop_and_exit_payout(msg),
            OpState::Idle => {
                if let Some(msg) = msg {
                    env::log_str(format!("Operation stopped: {msg:?}").as_str());
                }
                self.op_state = OpState::Idle;
            }
        }
        PromiseOrValue::Value(())
    }

    #[private]
    pub fn after_skim_balance(
        &mut self,
        #[callback_result] balance: Result<U128, PromiseError>,
        token: AccountId,
        recipient: AccountId,
    ) -> PromiseOrValue<()> {
        let amount = match balance {
            Ok(U128(v)) if v > 0 => v,
            _ => {
                // Invariant: Skim does nothing for zero balance (no-op cross-call avoided).
                env::log_str(&format!(
                    "Tried to skim; token={token}, recipient={recipient}"
                ));
                return PromiseOrValue::Value(());
            }
        };
        if amount == 0 {
            PromiseOrValue::Value(())
        } else {
            PromiseOrValue::Promise(
                ext_ft_core::ext(token)
                    .with_attached_deposit(NearToken::from_yoctonear(1))
                    .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
                    .ft_transfer(recipient, U128(amount), None),
            )
        }
    }
}
