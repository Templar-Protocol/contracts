use std::fmt::Display;

use crate::{ext_self, near, Contract, ContractExt, Error, Nep141Controller, OpState};
use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{
    env, json_types::U128, serde_json, AccountId, Gas, NearToken, Promise, PromiseError,
    PromiseOrValue,
};
use near_sdk_contract_tools::ft::nep141::GAS_FOR_FT_TRANSFER_CALL;
use templar_common::{market::ext_market, supply::SupplyPosition, vault::Event};

#[near]
impl Contract {
    pub const AFTER_SUPPLY_ENSURE_GAS: Gas = Gas::from_tgas(30);

    #[private]
    pub fn after_supply_1_check(
        &mut self,
        #[callback_result] accepted: Result<U128, PromiseError>, // NOTE: we probably can't rely on
        // this as a `true` value of accepted, so we are taking a belt-and-braces approach of
        // querying the supply position
        op_id: u64,
        market_index: u32,
        attempted: U128,
    ) -> PromiseOrValue<()> {
        // Invariant: Index drift or stale op_id results in a graceful stop
        match &self.op_state {
            OpState::Allocating { op_id: cur, .. } if *cur == op_id => {}
            _ => return self.stop_and_exit(Some(&Error::NotAllocating(self.op_state.clone()))),
        }

        // Resolve market by plan  or supply_queue
        let market: AccountId = if let Some(plan) = &self.plan {
            if let Some((m, _)) = plan.get(market_index as usize) {
                m.clone()
            } else {
                return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
            }
        } else if let Some(m) = self.supply_queue.get(market_index) {
            m.clone()
        } else {
            return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
        };

        // If the transfer failed, do not attempt to reconcile; stop and leave remaining untouched
        if accepted.is_err() {
            Event::AllocationTransferFailed {
                op_id,
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
                .with_static_gas(Self::GET_SUPPLY_POSITION_GAS)
                .with_unused_gas_weight(0)
                .get_supply_position(env::current_account_id())
                .then(
                    ext_self::ext(env::current_account_id())
                        .with_static_gas(Self::AFTER_SUPPLY_POSITION_CHECK_GAS)
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

    pub const GET_SUPPLY_POSITION_GAS: Gas = Gas::from_tgas(4);
    pub const AFTER_SUPPLY_POSITION_CHECK_GAS: Gas = Gas::from_tgas(10);
    // FIXME: no panics in this function! This will cause to spin if the op changes
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

        // Resolve market by plan (if present) or supply_queue
        let market: AccountId = if let Some(plan) = &self.plan {
            if let Some((m, _)) = plan.get(market_index as usize) {
                m.clone()
            } else {
                return self.stop_and_exit(Some(&Error::MissingMarket(market_index)));
            }
        } else if let Some(m) = self.supply_queue.get(market_index) {
            m.clone()
        } else {
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
                Event::AllocationPositionMissing {
                    op_id,
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
                    op_id,
                    index: market_index,
                    market: market.clone(),
                    attempted,
                    accepted,
                }
                .emit();
                return self.stop_and_exit(Some(&Error::PositionReadFailed));
            }
        };

        // Emit step settled event
        let accepted_event = new_principal.saturating_sub(before.0);
        // Compute refund from ground truth (attempted - accepted), ignoring token-reported value
        let refunded = attempted.0.saturating_sub(accepted_event);
        Event::AllocationStepSettled {
            op_id,
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

        self.market_supply.insert(market.clone(), new_principal);
        // Invariant: withdraw_queue gains any market with new_principal > 0
        if new_principal > 0 && !self.withdraw_queue.iter().any(|m| m == &market) {
            self.withdraw_queue.push(market.clone());
        }

        self.op_state = OpState::Allocating {
            op_id,
            index: market_index + 1,
            remaining: remaining_next,
        };
        self.step_allocation()
    }

    pub const AFTER_CREATE_WITHDRAW_REQ_GAS: Gas = Gas::from_tgas(20);

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
                    .with_static_gas(GAS_FOR_FT_TRANSFER_CALL)
                    // TODO: we can only do this if there is sufficient liquidity in the market, we
                    // should check that there is first, but even so, we can be rugged
                    .execute_next_supply_withdrawal_request()
                    .then(
                        ext_self::ext(env::current_account_id())
                            .with_static_gas(Self::AFTER_CREATE_WITHDRAW_REQ_GAS)
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
                    ext_self::ext(env::current_account_id())
                        .with_static_gas(Self::AFTER_CREATE_WITHDRAW_REQ_GAS)
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
                Event::WithdrawalPositionMissing {
                    op_id,
                    market: market.clone(),
                    index: market_index,
                    before: U128(before_principal),
                    need,
                }
                .emit();
                0
            }
            Err(_) => {
                Event::WithdrawalPositionReadFailed {
                    op_id,
                    market: market.clone(),
                    index: market_index,
                    before: U128(before_principal),
                    need,
                }
                .emit();
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
                    burn_shares: escrow_shares,
                };
                PromiseOrValue::Promise(
                    self.underlying_asset
                        .clone()
                        .transfer(recv.clone(), U128(collected).into())
                        .then(
                            ext_self::ext(env::current_account_id())
                                .with_static_gas(Self::AFTER_SEND_TO_USER_GAS)
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

    pub const AFTER_SEND_TO_USER_GAS: Gas = Gas::from_tgas(5);

    #[private]
    pub fn after_send_to_user(
        &mut self,
        #[callback_result] result: Result<(), PromiseError>,
        op_id: u64,
        receiver: AccountId,
        amount: U128,
    ) -> bool {
        let (owner, escrow_shares, payout_amount, burn_shares) = match &self.op_state {
            OpState::Payout {
                op_id: cur,
                receiver: r,
                amount: a,
                owner,
                escrow_shares,
                burn_shares,
            } if *cur == op_id && *r == receiver => {
                (owner.clone(), *escrow_shares, *a, *burn_shares)
            }
            _ => {
                Event::PayoutUnexpectedState {
                    op_id,
                    receiver: receiver.clone(),
                    amount,
                }
                .emit();
                return false;
            }
        };

        if let Ok(()) = result {
            // Invariant: On payout success, idle_balance -= payout_amount.
            // Burn only the proportional shares and refund the remainder to the owner.
            self.idle_balance = self.idle_balance.saturating_sub(payout_amount);
            let to_burn = burn_shares.min(escrow_shares);
            if to_burn > 0 {
                self.withdraw_unchecked(&env::current_account_id(), to_burn)
                    .expect("Failed to burn escrowed shares");
            }
            let refund_shares = escrow_shares.saturating_sub(to_burn);
            if refund_shares > 0 {
                #[allow(clippy::expect_used, reason = "No side effects")]
                self.transfer_unchecked(&env::current_account_id(), &owner, refund_shares)
                    .expect("Failed to refund remaining escrowed shares");
            }
            self.op_state = OpState::Idle;
            true
        } else {
            // Invariant: On payout failure, refund full escrow to owner and leave idle_balance unchanged
            #[allow(clippy::expect_used, reason = "No side effects")]
            self.transfer_unchecked(&env::current_account_id(), &owner, escrow_shares)
                .expect("Failed to release escrowed shares");
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

    fn stop_and_exit_allocating<T: Display + core::fmt::Debug + ?Sized>(
        &mut self,
        msg: Option<&T>,
    ) {
        // replaced log with events elsewhere; no-op here
        if let OpState::Allocating {
            op_id,
            index,
            remaining,
        } = &self.op_state
        {
            // Emit completion vs stop event before reconciling remaining
            match msg {
                None => {
                    Event::AllocationCompleted { op_id: *op_id }.emit();
                }
                Some(m) => {
                    Event::AllocationStopped {
                        op_id: *op_id,
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
    fn stop_and_exit_withdrawing<T: Display + core::fmt::Debug + ?Sized>(
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
                op_id,
                index,
                remaining: U128(remaining),
                collected: U128(collected),
                reason: msg.map(|m| m.to_string()),
            }
            .emit();
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
        {
            if let OpState::Payout {
                op_id,
                receiver,
                amount,
                ..
            } = &self.op_state
            {
                Event::PayoutStopped {
                    op_id: *op_id,
                    receiver: receiver.clone(),
                    amount: U128(*amount),
                    reason: msg.map(|m| m.to_string()),
                }
                .emit();
            }
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
                Event::OperationStoppedWhileIdle {
                    reason: msg.map(|m| format!("{m:?}")),
                }
                .emit();
                self.op_state = OpState::Idle;
            }
        }
        PromiseOrValue::Value(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Contract;
    use near_sdk::json_types::U128;
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::{test_utils::testing_env_with_promise_results, AccountId, PromiseOrValue};
    use near_sdk::{test_vm_config, testing_env, PromiseResult, RuntimeFeesConfig};
    use templar_common::asset::{BorrowAsset, FungibleAsset};
    use templar_common::vault::{AllocationMode, OpState, VaultConfiguration};
    use test_utils::vault_configuration;

    fn mk(n: u32) -> AccountId {
        format!("acc{n}.testnet").parse().expect("valid account id")
    }

    fn setup_env(
        current: &AccountId,
        predecessor: &AccountId,
        promise_results: Vec<PromiseResult>,
    ) {
        let mut builder = VMContextBuilder::new();
        builder.current_account_id(current.clone());
        builder.predecessor_account_id(predecessor.clone());
        builder.signer_account_id(predecessor.clone());
        testing_env!(
            builder.build(),
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            promise_results
        );
    }

    fn new_test_contract(vault_id: &AccountId) -> Contract {
        // Ensure env is available before constructing the contract (uses env::storage_usage etc).
        setup_env(vault_id, vault_id, vec![]);

        // Basic accounts
        let owner = accounts(1);
        let curator = accounts(2);
        let guardian = accounts(3);
        let fee_recipient = accounts(4);
        let skim_recipient = accounts(5);
        let underlying_token_id = mk(6);

        let cfg = vault_configuration(
            owner,
            curator,
            guardian,
            underlying_token_id,
            skim_recipient,
            fee_recipient,
        );

        Contract::new(cfg)
    }

    #[test]
    fn after_send_to_user_success_no_escrow() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

        let receiver = mk(7);

        // Seed idle balance and set Payout state; use zero escrow/burn to avoid FT side-effects in unit test.
        c.idle_balance = 1_000;
        c.op_state = OpState::Payout {
            op_id: 1,
            receiver: receiver.clone(),
            amount: 200,
            owner: accounts(1),
            escrow_shares: 0,
            burn_shares: 0,
        };

        // Provide a successful callback result
        let ok = c.after_send_to_user(Ok(()), 1, receiver.clone(), U128(200));
        assert!(ok, "Payout should report success");
        assert_eq!(c.idle_balance, 800, "Idle balance must decrease by payout");
        assert!(
            matches!(c.op_state, OpState::Idle),
            "Vault must go Idle after successful payout"
        );
    }

    #[test]
    fn after_exec_withdraw_read_none_to_payout() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

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

        // Should schedule payout (Promise) after crediting and zeroing remaining
        match res {
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Expected a Promise to send payout"),
        }

        // Market principal should be zeroed
        assert_eq!(
            *c.market_supply.get(&market).unwrap_or(&u128::MAX),
            0,
            "Market principal should be updated to 0"
        );

        // Idle balance should be credited by 60
        assert_eq!(
            c.idle_balance, 60,
            "Idle balance should increase by credited amount"
        );

        // State should transition to Payout with amount = collected (10) + credited (60) = 70
        match &c.op_state {
            OpState::Payout { amount, .. } => {
                assert_eq!(*amount, 70, "Payout amount must match collected + credited");
            }
            other => panic!("Unexpected state after read: {:?}", other),
        }
    }

    #[test]
    fn after_skim_balance_zero_noop() {
        let vault_id = accounts(0);
        setup_env(&vault_id, &vault_id, vec![]);

        let mut c = new_test_contract(&vault_id);

        // Zero balance -> Value(())
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
            PromiseOrValue::Promise(_) => {}
            _ => panic!("Skim with positive balance must return a Promise"),
        }
    }
}
