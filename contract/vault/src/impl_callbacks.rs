use std::fmt::Display;

use crate::{
    ext_self, near, Contract, ContractExt, Error, EscrowSettlement, Nep141Controller, OpState,
};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, json_types::U128, AccountId, NearToken, PromiseError, PromiseOrValue};
use near_sdk_contract_tools::ft::nep141::GAS_FOR_FT_TRANSFER_CALL;
use templar_common::{
    market::ext_market,
    supply::SupplyPosition,
    vault::{
        Event, AFTER_CREATE_WITHDRAW_REQ_GAS, AFTER_EXEC_WITHDRAW_READ_GAS, AFTER_SEND_TO_USER_GAS,
        AFTER_SUPPLY_POSITION_CHECK_GAS, EXECUTE_WITHDRAW_REQ_GAS, GET_SUPPLY_POSITION_GAS,
    },
};

#[near]
impl Contract {
    #[private]
    pub fn after_supply_1_check(
        &mut self,
        // NOTE: we can't rely on this as a `true` value of accepted, so we are taking a belt-and-braces approach of
        // querying the supply position
        #[callback_result] accepted: Result<U128, PromiseError>,
        op_id: u64,
        market_index: u32,
        attempted: U128,
    ) -> PromiseOrValue<()> {
        if let Err(e) = self.ctx_allocating(op_id) {
            return self.stop_and_exit(Some(&e));
        };

        let market = match self.resolve_supply_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        // If the transfer failed, do not attempt to reconcile; stop and leave remaining untouched
        if accepted.is_err() {
            Event::AllocationTransferFailed {
                op_id: op_id.into(),
                index: market_index,
                market: market.clone(),
                attempted,
            }
            .emit();
            return self.stop_and_exit(Some(&Error::MarketTransferFailed));
        }

        let before = self.market_supply.get(&market).unwrap_or(&0);

        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(GET_SUPPLY_POSITION_GAS)
                .with_unused_gas_weight(0)
                .get_supply_position(env::current_account_id())
                .then(
                    ext_self::ext(env::current_account_id())
                        .with_static_gas(AFTER_SUPPLY_POSITION_CHECK_GAS)
                        .after_supply_2_read(
                            op_id,
                            market_index,
                            U128(*before),
                            attempted,
                            accepted.unwrap_or(U128(0)),
                        ),
                ),
        )
    }

    #[private]
    pub fn after_supply_2_read(
        &mut self,
        #[callback_result] position: Result<Option<SupplyPosition>, PromiseError>,
        op_id: u64,
        market_index: u32,
        before: U128,
        attempted: U128,
        accepted: U128,
    ) -> PromiseOrValue<()> {
        let (i, remaining) = match self.ctx_allocating(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if i != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(i, market_index)));
        }

        let market = match self.resolve_supply_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        let SupplyReconciliation {
            new_principal,
            accepted_event,
            remaining,
        } = match position {
            Ok(Some(position)) => reconcile_supply_outcome(
                &position.get_deposit().total().into(),
                &before.0,
                &remaining,
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
            remaining_after: U128(remaining),
        }
        .emit();

        self.market_supply.insert(market.clone(), new_principal);

        // Invariant: withdraw_queue gains any market with new_principal > 0
        if new_principal > 0 {
            self.add_market_to_withdraw_queue(&market, before.0);
        }

        self.op_state = OpState::Allocating {
            op_id,
            index: market_index + 1,
            remaining,
        };
        self.step_allocation()
    }

    #[private]
    pub fn after_create_withdraw_req(
        &mut self,
        #[callback_result] did_create: Result<(), PromiseError>,
        op_id: u64,
        market_index: u32,
        need: U128,
    ) -> PromiseOrValue<()> {
        let (i, remaining, received, collected, owner, escrow_shares) =
            match self.ctx_withdrawing(op_id) {
                Ok(v) => v,
                Err(e) => return self.stop_and_exit(Some(&e)),
            };

        if i != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(i, market_index)));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if did_create.is_ok() {
            PromiseOrValue::Promise(
                ext_market::ext(market.clone())
                    .with_static_gas(EXECUTE_WITHDRAW_REQ_GAS)
                    .with_unused_gas_weight(0)
                    .execute_next_supply_withdrawal_request()
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(AFTER_CREATE_WITHDRAW_REQ_GAS)
                            .after_exec_withdraw_req(op_id, market_index, need),
                    ),
            )
        } else {
            env::log_str("create_supply_withdrawal_request failed; moving to next market");
            self.op_state = OpState::Withdrawing {
                op_id,
                index: market_index + 1,
                remaining: remaining,
                receiver: received,
                collected: collected,
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
        let (i, _, _, _, _, _) = match self.ctx_withdrawing(op_id) {
            Ok(v) => v,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        if i != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(i, market_index)));
        }

        let market = match self.resolve_withdraw_market(market_index) {
            Ok(m) => m,
            Err(e) => return self.stop_and_exit(Some(&e)),
        };

        // Verify actual withdrawal by reading market position after execution
        let before = *self.market_supply.get(&market).unwrap_or(&0);
        PromiseOrValue::Promise(
            ext_market::ext(market.clone())
                .with_static_gas(GET_SUPPLY_POSITION_GAS)
                .with_unused_gas_weight(0)
                .get_supply_position(env::current_account_id())
                .then(
                    ext_self::ext(env::current_account_id())
                        .with_static_gas(AFTER_EXEC_WITHDRAW_READ_GAS)
                        .after_exec_withdraw_read(op_id, market_index, U128(before), need),
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
        let (i, remaining, receiver, collected, owner, escrow_shares) =
            match self.ctx_withdrawing(op_id) {
                Ok(v) => v,
                Err(e) => return self.stop_and_exit(Some(&e)),
            };

        if i != market_index {
            return self.stop_and_exit(Some(&Error::IndexDrifted(i, market_index)));
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

        let (_credited, remaining, collected, idle_delta) = self.reconcile_withdraw_outcome(
            before_principal,
            new_principal,
            need.0,
            remaining,
            collected,
        );

        self.market_supply.insert(market.clone(), new_principal);
        if idle_delta > 0 {
            self.idle_balance = self.idle_balance.saturating_add(idle_delta);
        }

        if remaining == 0 {
            if collected > 0 {
                self.op_state = OpState::Payout {
                    op_id,
                    receiver: receiver.clone(),
                    amount: collected,
                    owner: owner.clone(),
                    escrow_shares,
                    burn_shares: escrow_shares,
                };
                PromiseOrValue::Promise(
                    self.underlying_asset
                        .clone()
                        .transfer(receiver.clone(), U128(collected).into())
                        .then(
                            ext_self::ext(env::current_account_id())
                                .with_static_gas(AFTER_SEND_TO_USER_GAS)
                                .after_send_to_user(op_id, receiver, U128(collected)),
                        ),
                )
            } else {
                // Nothing collected; refund escrowed shares
                let self_id = env::current_account_id();
                // We expect the owner to maintain storage accounts, otherwise they will lose access to their funds
                self.transfer_unchecked(&self_id, &owner, escrow_shares)
                    .expect("Failed to refund escrowed shares");
                self.op_state = OpState::Idle;
                PromiseOrValue::Value(())
            }
        } else {
            self.op_state = OpState::Withdrawing {
                op_id,
                index: market_index + 1,
                remaining,
                receiver: receiver,
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
        let (owner, escrow_shares, amount, burn_shares) = match &self.op_state {
            OpState::Payout {
                op_id: current_op,
                receiver: recv,
                amount,
                owner,
                escrow_shares,
                burn_shares,
            } if *current_op == op_id && *recv == receiver => {
                (owner.clone(), *escrow_shares, *amount, *burn_shares)
            }
            _ => {
                Event::PayoutUnexpectedState {
                    op_id: op_id.into(),
                    receiver: receiver.clone(),
                    amount,
                }
                .emit();
                return false;
            }
        };

        if result.is_ok() {
            // On payout success, idle_balance -= payout_amount.
            // Burn only the proportional shares and refund the remainder to the owner.
            self.idle_balance = self.idle_balance.saturating_sub(amount);
            let EscrowSettlement { to_burn, refund } =
                Self::compute_escrow_settlement(escrow_shares, burn_shares);
            if to_burn > 0 {
                self.withdraw_unchecked(&env::current_account_id(), to_burn)
                    .unwrap_or_else(|e| env::panic_str(&e.to_string()));
            }
            if refund > 0 {
                #[allow(clippy::expect_used, reason = "No side effects")]
                self.transfer_unchecked(&env::current_account_id(), &owner, refund)
                    .unwrap_or_else(|e| env::panic_str(&e.to_string()));
            }
            self.op_state = OpState::Idle;
            true
        } else {
            // On payout failure, refund full escrow to owner and leave idle_balance unchanged
            #[allow(clippy::expect_used, reason = "No side effects")]
            self.transfer_unchecked(&env::current_account_id(), &owner, escrow_shares)
                .unwrap_or_else(|e| env::panic_str(&e.to_string()));
            self.op_state = OpState::Idle;
            false
        }
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
                // Invariant: Skim does nothing for zero balance
                Event::SkimNoop {
                    token: token.clone(),
                    recipient: recipient.clone(),
                }
                .emit();
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

impl Contract {
    pub fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        if let OpState::Allocating {
            op_id,
            index,
            remaining,
        } = &self.op_state
        {
            match msg {
                None => {
                    Event::AllocationCompleted { op_id: *op_id }.emit();
                }
                Some(m) => {
                    Event::AllocationStopped {
                        op_id: (*op_id).into(),
                        index: *index,
                        remaining: U128(*remaining),
                        reason: Some(m.to_string()),
                    }
                    .emit();
                }
            }

            if *remaining > 0 {
                self.idle_balance = self.idle_balance.saturating_add(*remaining);
            }
        }
        self.plan = None;
        self.op_state = OpState::Idle;
    }

    /// Stop helper for Withdrawing: refund escrowed shares to owner and go Idle.
    pub fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        {
            let (op_id, index, remaining, collected) = match &self.op_state {
                OpState::Withdrawing {
                    op_id,
                    index,
                    remaining,
                    collected,
                    ..
                } => (*op_id, *index, *remaining, *collected),
                _ => (0, 0, 0, 0),
            };
            Event::WithdrawalStopped {
                op_id: op_id.into(),
                index,
                remaining: U128(remaining),
                collected: U128(collected),
                reason: msg.map(std::string::ToString::to_string),
            }
            .emit();
        }
        if let Some((owner_acc, escrow)) = match &self.op_state {
            OpState::Withdrawing {
                owner,
                escrow_shares,
                ..
            } if *escrow_shares > 0 => Some((owner.clone(), *escrow_shares)),
            _ => None,
        } {
            let self_id = env::current_account_id();
            #[allow(clippy::expect_used, reason = "No side effects")]
            self.transfer_unchecked(&self_id, &owner_acc, escrow)
                .unwrap_or_else(|e| env::panic_str(&e.to_string()));
        }
        self.op_state = OpState::Idle;
    }

    /// refund escrowed shares to owner and go Idle.
    pub fn stop_and_exit_payout<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        {
            if let OpState::Payout {
                op_id,
                receiver,
                amount,
                ..
            } = &self.op_state
            {
                Event::PayoutStopped {
                    op_id: (*op_id).into(),
                    receiver: receiver.clone(),
                    amount: U128(*amount),
                    reason: msg.map(std::string::ToString::to_string),
                }
                .emit();
            }
        }
        if let OpState::Payout {
            owner,
            escrow_shares,
            ..
        } = &self.op_state
        {
            if *escrow_shares > 0 {
                let self_id = env::current_account_id();
                let owner_acc = owner.clone();
                let escrow = *escrow_shares;
                #[allow(clippy::expect_used, reason = "No side effects")]
                self.transfer_unchecked(&self_id, &owner_acc, escrow)
                    .unwrap_or_else(|e| env::panic_str(&e.to_string()));
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
                Event::OperationStoppedWhileIdle {
                    reason: msg.map(|m| format!("{m:?}")),
                }
                .emit();
                self.op_state = OpState::Idle;
            }
        }
        PromiseOrValue::Value(())
    }

    /// Validate current op is Allocating and return (index, remaining)
    pub(crate) fn ctx_allocating(&self, op_id: u64) -> Result<(u32, u128), Error> {
        match &self.op_state {
            OpState::Allocating {
                op_id: cur,
                index,
                remaining,
            } if *cur == op_id => Ok((*index, *remaining)),
            _ => Err(Error::NotAllocating),
        }
    }

    /// Validate current op is Withdrawing and return context tuple
    pub(crate) fn ctx_withdrawing(
        &self,
        op_id: u64,
    ) -> Result<(u32, u128, AccountId, u128, AccountId, u128), Error> {
        match &self.op_state {
            OpState::Withdrawing {
                op_id: cur,
                index,
                remaining,
                receiver,
                collected,
                owner,
                escrow_shares,
            } if *cur == op_id => Ok((
                *index,
                *remaining,
                receiver.clone(),
                *collected,
                owner.clone(),
                *escrow_shares,
            )),
            _ => Err(Error::NotWithdrawing),
        }
    }

    /// Resolve a market for allocation by plan (if present) or supply_queue
    pub(crate) fn resolve_supply_market(&self, market_index: u32) -> Result<AccountId, Error> {
        if let Some(plan) = &self.plan {
            if let Some((m, _)) = plan.get(market_index as usize) {
                return Ok(m.clone());
            }
            return Err(Error::MissingMarket(market_index));
        }
        self.supply_queue
            .get(market_index)
            .cloned()
            .ok_or(Error::MissingMarket(market_index))
    }

    /// Resolve a market for withdraw by withdraw_queue
    pub(crate) fn resolve_withdraw_market(&self, market_index: u32) -> Result<AccountId, Error> {
        self.withdraw_queue
            .get(market_index)
            .cloned()
            .ok_or(Error::MissingMarket(market_index))
    }

    /// Pure reconciliation for withdraw read outcome to enable unit tests
    pub(crate) fn reconcile_withdraw_outcome(
        &self,
        before_principal: u128,
        new_principal: u128,
        need: u128,
        rem: u128,
        coll: u128,
    ) -> (
        u128, /* credited */
        u128, /* remaining_next */
        u128, /* collected_next */
        u128, /* idle_delta */
    ) {
        let withdrawn = before_principal.saturating_sub(new_principal);
        let credited = withdrawn.min(need);
        let remaining_next = rem.saturating_sub(credited);
        let collected_next = coll.saturating_add(credited);
        let idle_delta = credited;
        (credited, remaining_next, collected_next, idle_delta)
    }
}

pub(crate) struct SupplyReconciliation {
    new_principal: u128,
    accepted_event: u128,
    remaining: u128,
}

pub(crate) fn reconcile_supply_outcome(
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

#[cfg(test)]
mod tests {
    use std::u128;

    use crate::impl_callbacks::reconcile_supply_outcome;
    use crate::test_utils::*;

    use near_sdk::json_types::U128;
    use near_sdk::test_utils::accounts;
    use near_sdk::PromiseOrValue;
    use near_sdk::PromiseResult;
    use rstest::rstest;

    use crate::Contract;
    use near_sdk::AccountId;
    use rstest::fixture;
    use templar_common::vault::Error;
    use templar_common::vault::OpState;

    #[fixture]
    fn vault_id() -> AccountId {
        accounts(0)
    }

    #[fixture]
    fn c(vault_id: AccountId) -> Contract {
        setup_env(&vault_id, &vault_id, vec![]);
        new_test_contract(&vault_id)
    }

    // Contract with the env used by after_supply_1_check_* tests
    #[fixture]
    fn c_max(vault_id: AccountId) -> Contract {
        setup_env(
            &vault_id,
            &vault_id,
            vec![PromiseResult::Successful(
                near_sdk::serde_json::to_vec(&U128(u128::MAX))
                    .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
            )],
        );
        new_test_contract(&vault_id)
    }

    #[fixture]
    fn receiver() -> AccountId {
        mk(9)
    }

    #[fixture]
    fn owner() -> AccountId {
        accounts(1)
    }

    #[rstest]
    fn after_supply_1_check_allocating_not_allocating(mut c_max: Contract) {
        let mut c = c_max;

        c.op_state = OpState::Idle;

        c.after_supply_1_check(Ok(U128(1)), 0, 2, Default::default());

        assert_eq!(c.op_state, OpState::Idle);
        assert_eq!(c.plan, None);
    }

    #[test]
    fn after_supply_1_check_allocating_not_allocating_index() {
        let vault_id = accounts(0);
        setup_env(
            &vault_id,
            &vault_id,
            vec![PromiseResult::Successful(
                near_sdk::serde_json::to_vec(&U128(u128::MAX))
                    .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
            )],
        );

        let mut c = new_test_contract(&vault_id);

        let op_id = 1;
        let receiver = mk(7);

        c.op_state = OpState::Allocating {
            op_id,
            index: 0u32,
            remaining: 0u128,
        };

        c.after_supply_1_check(Ok(U128(1)), op_id + 1, 0, Default::default());

        assert_eq!(c.op_state, OpState::Idle);
        assert_eq!(c.plan, None);
    }

    #[test]
    fn after_supply_1_check_allocating() {
        let vault_id = accounts(0);
        setup_env(
            &vault_id,
            &vault_id,
            vec![PromiseResult::Successful(
                near_sdk::serde_json::to_vec(&U128(u128::MAX))
                    .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string())),
            )],
        );

        let mut c = new_test_contract(&vault_id);

        let op_id = 1;
        let receiver = mk(7);

        c.op_state = OpState::Allocating {
            op_id,
            index: 0u32,
            remaining: 0u128,
        };

        c.after_supply_1_check(Ok(U128(1)), op_id, 0, Default::default());

        assert_eq!(c.op_state, OpState::Idle);
        assert_eq!(c.plan, None);
    }

    #[test]
    fn after_send_to_user_success_no_escrow() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

        let receiver = mk(7);

        c.idle_balance = 1_000;
        c.op_state = OpState::Payout {
            op_id: 1,
            receiver: receiver.clone(),
            amount: 200,
            owner: accounts(1),
            escrow_shares: 0,
            burn_shares: 0,
        };

        let ok = c.after_send_to_user(Ok(()), 1, receiver.clone(), U128(200));
        assert!(ok, "Payout should report success");
        assert_eq!(c.idle_balance, 800, "Idle balance must decrease by payout");
        assert!(
            matches!(c.op_state, OpState::Idle),
            "Vault must go Idle after successful payout"
        );
    }

    #[rstest]
    fn after_exec_withdraw_read_none_to_payout(mut c: Contract) {
        // Prepare a single-market withdraw queue with non-zero principal
        let market = mk(8);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), 100);

        // Withdrawing: need 60, already collected 10; expect position None => new_principal = 0, withdrawn = 100, credited = min(100, 60) = 60
        c.op_state = OpState::Withdrawing {
            op_id: 42,
            index: 0,
            remaining: 60,
            receiver: mk(9),
            collected: 10,
            owner: accounts(1),
            escrow_shares: 50,
        };

        let res = c.after_exec_withdraw_read(Ok(None), 42, 0, U128(100), U128(60));

        match res {
            PromiseOrValue::Promise(p) => {}
            _ => panic!("Expected a Promise to send payout"),
        }

        assert_eq!(
            *c.market_supply.get(&market).unwrap_or(&u128::MAX),
            0,
            "Market principal should be updated to 0"
        );

        assert_eq!(
            c.idle_balance, 60,
            "Idle balance should increase by credited amount"
        );

        // State should transition to Payout with amount = collected (10) + credited (60) = 70
        match &c.op_state {
            OpState::Payout { amount, .. } => {
                assert_eq!(*amount, 70, "Payout amount must match collected + credited");
            }
            other => panic!("Unexpected state after read: {other:?}"),
        }
    }

    #[test]
    fn after_skim_balance_zero_noop() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

        let res = c.after_skim_balance(Ok(U128(0)), mk(10), mk(11));
        match res {
            PromiseOrValue::Value(()) => {}
            _ => panic!("Skim with zero balance must be a no-op"),
        }
    }

    #[test]
    fn after_skim_balance_positive_returns_promise() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

        // Positive balance -> Promise to ft_transfer
        let res = c.after_skim_balance(Ok(U128(123)), mk(10), mk(11));
        match res {
            PromiseOrValue::Promise(_) => { //NOTE: one day we will be able to read the promise
                 //definition :<
            }
            _ => panic!("Skim with positive balance must return a Promise"),
        }
    }

    /// Property: Payout failure keeps idle_balance unchanged and does not burn escrow
    #[rstest(
        idle => [0u128, 1, 100],
        escrow => [0u128, 1, 50],
        amount => [0u128, 1, 25]
    )]
    fn prop_after_send_to_user_failure_keeps_idle(idle: u128, escrow: u128, amount: u128) {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        let receiver = mk(7);
        let owner = accounts(1);

        if escrow > 0 {
            use near_sdk_contract_tools::ft::Nep141Controller as _;

            c.deposit_unchecked(&near_sdk::env::current_account_id(), escrow)
                .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string()));
        }

        c.idle_balance = idle;
        c.op_state = OpState::Payout {
            op_id: 1,
            receiver: receiver.clone(),
            amount,
            owner: owner.clone(),
            escrow_shares: escrow,
            burn_shares: escrow,
        };

        let before = c.idle_balance;
        let ok = c.after_send_to_user(
            Err(near_sdk::PromiseError::Failed),
            1,
            receiver.clone(),
            U128(amount),
        );
        assert!(!ok, "Payout failure should return false");
        assert_eq!(
            c.idle_balance, before,
            "idle_balance must stay the same on payout failure"
        );
        assert!(
            matches!(c.op_state, OpState::Idle),
            "Vault must go Idle after payout failure"
        );
    }

    /// Property: Create-withdraw failure skips to next market and if collected>0 ends in Payout
    #[rstest(
        collected => [1u128, 10u128],
        need => [1u128, 5u128]
    )]
    fn prop_after_create_withdraw_req_failure_skips(collected: u128, need: u128) {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        // Single-market queue so advancing index reaches end-of-queue
        let market = mk(8);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), 100);

        c.op_state = OpState::Withdrawing {
            op_id: 7,
            index: 0,
            remaining: need,
            receiver: mk(9),
            collected,
            owner: accounts(1),
            escrow_shares: 0,
        };

        let res =
            c.after_create_withdraw_req(Err(near_sdk::PromiseError::Failed), 7, 0, U128(need));
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected Promise after skipping to payout at end-of-queue"),
        }

        match &c.op_state {
            OpState::Payout { amount, .. } => {
                assert_eq!(*amount, collected, "Payout amount must equal collected");
            }
            other => panic!("Unexpected state: {other:?}"),
        }
    }

    /// Property: Exec-withdraw read failure assumes unchanged principal and does not credit idle
    #[rstest(
        before => [0u128, 1u128, 100u128],
        need => [0u128, 1u128, 50u128],
        collected => [1u128, 2u128]
    )]
    fn prop_after_exec_withdraw_read_err_no_change(before: u128, need: u128, collected: u128) {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        let market = mk(8);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), before);

        let initial_idle = c.idle_balance;

        c.op_state = OpState::Withdrawing {
            op_id: 99,
            index: 0,
            remaining: need,
            receiver: mk(9),
            collected,
            owner: accounts(1),
            escrow_shares: 0,
        };

        let res = c.after_exec_withdraw_read(
            Err(near_sdk::PromiseError::Failed),
            99,
            0,
            U128(before),
            U128(need),
        );
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected Promise to send payout at end-of-queue"),
        }

        assert_eq!(
            *c.market_supply.get(&market).unwrap_or(&u128::MAX),
            before,
            "principal must remain unchanged on read failure"
        );
        assert_eq!(
            c.idle_balance, initial_idle,
            "idle_balance must not change when nothing credited"
        );

        match &c.op_state {
            OpState::Payout { amount, .. } => {
                assert_eq!(*amount, collected, "Payout amount must equal collected");
            }
            other => panic!("Unexpected state: {other:?}"),
        }
    }

    /// Property: Callbacks must match current op_id or index; otherwise stop and go Idle
    #[rstest(
        pass_op => [false, true],
        pass_index => [false, true]
    )]
    fn prop_after_exec_withdraw_read_requires_current_state(pass_op: bool, pass_index: bool) {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        let market = mk(8);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), 10);

        let real_op = 5u64;
        let real_idx = 0u32;

        c.op_state = OpState::Withdrawing {
            op_id: real_op,
            index: real_idx,
            remaining: 1,
            receiver: mk(9),
            collected: 1,
            owner: accounts(1),
            escrow_shares: 0,
        };

        let call_op = if pass_op { real_op } else { real_op + 1 };
        let call_idx = if pass_index { real_idx } else { real_idx + 1 };

        let r = c.after_exec_withdraw_read(Ok(None), call_op, call_idx, U128(10), U128(1));
        if let (true, true) = (pass_op, pass_index) {
            assert!(
                !matches!(c.op_state, OpState::Idle),
                "Valid callback should not immediately stop"
            );
        } else {
            // Any mismatch should stop and go Idle
            if let PromiseOrValue::Value(()) = r {}
            assert!(
                matches!(c.op_state, OpState::Idle),
                "Mismatched callback must stop and go Idle"
            );
        }
    }

    #[test]
    fn refund_path_consistency() {
        use near_sdk_contract_tools::ft::Nep141Controller as _;

        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        // Seed escrowed shares into the vault's own account
        let owner = accounts(1);
        c.deposit_unchecked(&near_sdk::env::current_account_id(), 10)
            .unwrap_or_else(|e| near_sdk::env::panic_str(&e.to_string()));

        // Single-market withdraw queue (not used functionally here, just to satisfy path)
        let market = mk(12);
        c.withdraw_queue.push(market);

        // Withdrawing state with remaining=0 and collected=0 forces refund path
        c.op_state = OpState::Withdrawing {
            op_id: 77,
            index: 0,
            remaining: 0,
            receiver: mk(9),
            collected: 0,
            owner: owner.clone(),
            escrow_shares: 10,
        };

        let supply_before = c.total_supply();
        let vault_before = c.balance_of(&near_sdk::env::current_account_id());
        let owner_before = c.balance_of(&owner);

        // Read result with need=0 ensures credited=0; triggers refund branch
        let res = c.after_exec_withdraw_read(Ok(None), 77, 0, U128(0), U128(0));
        match res {
            PromiseOrValue::Value(()) => {}
            _ => panic!("Expected Value(()) on immediate escrow refund"),
        }

        // No burn/mint => total supply unchanged
        assert_eq!(
            c.total_supply(),
            supply_before,
            "no supply change on refund"
        );
        // Escrow shares transferred back to owner
        assert_eq!(
            c.balance_of(&near_sdk::env::current_account_id()),
            vault_before.saturating_sub(10),
            "vault should lose refunded escrow"
        );
        assert_eq!(
            c.balance_of(&owner),
            owner_before.saturating_add(10),
            "owner should receive refunded escrow"
        );
        // Vault returns to Idle
        assert!(
            matches!(c.op_state, OpState::Idle),
            "Vault must go Idle after refund"
        );
    }

    #[test]
    fn ctx_allocating_ok_and_err() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        c.op_state = OpState::Allocating {
            op_id: 42,
            index: 3,
            remaining: 77,
        };

        let ok = c.ctx_allocating(42).expect("ctx_allocating should succeed");
        assert_eq!(ok, (3, 77));

        // Wrong op_id => error
        assert!(c.ctx_allocating(43).is_err());
    }

    #[test]
    fn ctx_withdrawing_ok_and_err() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        let recv = mk(1);
        let owner = accounts(1);

        c.op_state = OpState::Withdrawing {
            op_id: 7,
            index: 1,
            remaining: 50,
            receiver: recv.clone(),
            collected: 5,
            owner: owner.clone(),
            escrow_shares: 10,
        };

        let (idx, rem, r, coll, o, escrow) = c
            .ctx_withdrawing(7)
            .expect("ctx_withdrawing should succeed");
        assert_eq!(idx, 1);
        assert_eq!(rem, 50);
        assert_eq!(r, recv);
        assert_eq!(coll, 5);
        assert_eq!(o, owner);
        assert_eq!(escrow, 10);

        // Wrong op_id => error
        assert!(c.ctx_withdrawing(8).is_err());
    }

    #[test]
    fn resolve_market_helpers_supply_and_withdraw() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        // Prepare markets
        let m1 = mk(1001);
        let m2 = mk(1002);

        // Supply: plan takes precedence
        c.plan = Some(vec![(m2.clone(), 1u128)]);
        c.supply_queue.push(m1.clone());
        c.supply_queue.push(m2.clone());

        assert_eq!(c.resolve_supply_market(0).unwrap(), m2);
        assert!(matches!(
            c.resolve_supply_market(1),
            Err(Error::MissingMarket(1))
        ));

        // Without plan, use queue
        c.plan = None;
        assert_eq!(c.resolve_supply_market(0).unwrap(), m1);
        assert_eq!(c.resolve_supply_market(1).unwrap(), m2);
        assert!(matches!(
            c.resolve_supply_market(2),
            Err(Error::MissingMarket(2))
        ));

        // Withdraw resolver uses withdraw_queue
        c.withdraw_queue.push(m1.clone());
        c.withdraw_queue.push(m2.clone());
        assert_eq!(c.resolve_withdraw_market(0).unwrap(), m1);
        assert_eq!(c.resolve_withdraw_market(1).unwrap(), m2);
        assert!(matches!(
            c.resolve_withdraw_market(2),
            Err(Error::MissingMarket(2))
        ));
    }

    #[test]
    fn after_supply_2_read_missing_position_stops() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        // Resolve market via supply_queue
        let market = mk(42);
        c.supply_queue.push(market);

        // Must be in Allocating ctx
        c.op_state = OpState::Allocating {
            op_id: 1,
            index: 0,
            remaining: 10,
        };

        // Missing position -> stop_and_exit
        let res = c.after_supply_2_read(Ok(None), 1, 0, U128(0), U128(5), U128(5));
        match res {
            PromiseOrValue::Value(()) => {}
            _ => panic!("Expected Value on missing position"),
        }
        assert!(matches!(c.op_state, OpState::Idle));
    }

    #[test]
    fn after_supply_2_read_read_failed_stops() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);
        let mut c = new_test_contract(&vault_id);

        // Resolve market via supply_queue
        let market = mk(43);
        c.supply_queue.push(market);

        // Must be in Allocating ctx
        c.op_state = OpState::Allocating {
            op_id: 7,
            index: 0,
            remaining: 100,
        };

        // Read failure -> stop_and_exit
        let res = c.after_supply_2_read(
            Err(near_sdk::PromiseError::Failed),
            7,
            0,
            U128(0),
            U128(10),
            U128(10),
        );
        match res {
            PromiseOrValue::Value(()) => {}
            _ => panic!("Expected Value on read failure"),
        }
        assert!(matches!(c.op_state, OpState::Idle));
    }

    #[rstest]
    fn after_create_withdraw_req_success_returns_promise(
        mut c: Contract,
        receiver: AccountId,
        owner: AccountId,
    ) {
        let market = mk(50);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), 100);

        c.op_state = OpState::Withdrawing {
            op_id: 21,
            index: 0,
            remaining: 60,
            receiver: receiver.clone(),
            collected: 10,
            owner: owner.clone(),
            escrow_shares: 5,
        };

        let res = c.after_create_withdraw_req(Ok(()), 21, 0, U128(60));
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected Promise when create succeeds"),
        }
        // State remains Withdrawing and will continue via the promise chain
        assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
    }

    #[rstest]
    fn after_exec_withdraw_req_returns_promise(mut c: Contract) {
        let market = mk(60);
        c.withdraw_queue.push(market.clone());
        c.market_supply.insert(market.clone(), 10);

        c.op_state = OpState::Withdrawing {
            op_id: 33,
            index: 0,
            remaining: 5,
            receiver: mk(9),
            collected: 0,
            owner: accounts(1),
            escrow_shares: 0,
        };

        let res = c.after_exec_withdraw_req(33, 0, U128(5));
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected Promise to read supply position after exec"),
        }
        assert!(matches!(c.op_state, OpState::Withdrawing { .. }));
    }

    #[rstest]
    fn after_exec_withdraw_read_advances_when_remaining(
        mut c: Contract,
        owner: AccountId,
        receiver: AccountId,
    ) {
        // Two markets; first has principal to withdraw
        let m1 = mk(70);
        let m2 = mk(71);
        c.withdraw_queue.push(m1.clone());
        c.withdraw_queue.push(m2.clone());
        c.market_supply.insert(m1.clone(), 10);

        c.op_state = OpState::Withdrawing {
            op_id: 0,
            index: 0,
            remaining: 100,
            receiver: receiver.clone(),
            collected: 0,
            owner: owner.clone(),
            escrow_shares: 0,
        };

        // Position None => new_principal = 0 => withdrawn = 10 => credited = 10
        let res = c.after_exec_withdraw_read(Ok(None), 0, 0, U128(10), U128(100));
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected Promise to continue withdraw steps"),
        }

        // Idle credited, state advanced to next index with remaining reduced
        assert_eq!(c.idle_balance, 10);

        // This works
        match &c.op_state {
            OpState::Payout {
                op_id,
                receiver: r,
                amount,
                owner: o,
                escrow_shares,
                burn_shares,
            } => {
                assert_eq!(*op_id, 0);
                assert_eq!(*amount, 10);
                assert_eq!(*escrow_shares, 0);
                assert_eq!(*burn_shares, 0);
                assert_eq!(*r, receiver);
                assert_eq!(*o, owner);
            }
            other => panic!("Unexpected state after advancing: {other:?}"),
        }
    }

    #[rstest]
    fn stop_and_exit_when_idle_emits_and_stays_idle(mut c: Contract) {
        // Already Idle; ensure branch is executed
        c.op_state = OpState::Idle;

        let res = c.stop_and_exit::<&str>(Some(&"reason"));
        match res {
            PromiseOrValue::Value(()) => {}
            _ => panic!("Expected Value on stop while Idle"),
        }
        assert!(matches!(c.op_state, OpState::Idle));
    }
    #[test]
    fn accepts_increase_and_decrements_remaining() {
        let out = reconcile_supply_outcome(&1_600, &1_000, &1_000);
        let expected_accepted = 1_600u128.saturating_sub(1_000);
        let expected_remaining = 1_000u128.saturating_sub(expected_accepted);

        assert_eq!(out.new_principal, 1_600);
        assert_eq!(out.accepted_event, expected_accepted); // 600
        assert_eq!(out.remaining, expected_remaining); // 400
    }

    #[test]
    fn no_accept_when_total_does_not_increase() {
        // decreased
        let out = reconcile_supply_outcome(&1_500, &2_000, &5_000);
        assert_eq!(out.new_principal, 1_500);
        assert_eq!(out.accepted_event, 0);
        assert_eq!(out.remaining, 5_000);

        // equal
        let out = reconcile_supply_outcome(&2_000, &2_000, &1_234);
        assert_eq!(out.new_principal, 2_000);
        assert_eq!(out.accepted_event, 0);
        assert_eq!(out.remaining, 1_234);
    }

    #[test]
    fn remaining_saturates_to_zero_when_acceptance_exceeds_it() {
        let out = reconcile_supply_outcome(&u128::MAX, &0, &1);
        assert_eq!(out.new_principal, u128::MAX);
        assert_eq!(out.accepted_event, u128::MAX);
        assert_eq!(out.remaining, 0);

        let out = reconcile_supply_outcome(&10_000, &0, &5);
        assert_eq!(out.new_principal, 10_000);
        assert_eq!(out.accepted_event, 10_000);
        assert_eq!(out.remaining, 0);
    }

    #[test]
    fn handles_extreme_boundaries_correctly() {
        let out = reconcile_supply_outcome(&0, &0, &0);
        assert_eq!(out.new_principal, 0);
        assert_eq!(out.accepted_event, 0);
        assert_eq!(out.remaining, 0);

        let out = reconcile_supply_outcome(&0, &u128::MAX, &123);
        assert_eq!(out.new_principal, 0);
        assert_eq!(out.accepted_event, 0);
        assert_eq!(out.remaining, 123);

        let out = reconcile_supply_outcome(&u128::MAX, &(u128::MAX - 5), &2);
        assert_eq!(out.new_principal, u128::MAX);
        assert_eq!(out.accepted_event, 5);
        assert_eq!(out.remaining, 0);
    }
}
