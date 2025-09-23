use near_sdk::{env, near, require, serde_json, AccountId, Gas, Promise, PromiseResult};
use templar_common::{
    asset::{
        BorrowAsset, BorrowAssetAmount, CollateralAsset, CollateralAssetAmount, FungibleAsset,
    },
    asset_op,
    borrow::{InitialBorrow, InitialLiquidation},
    market::{LiquidateMsg, WithdrawalResolution},
    oracle::pyth::OracleResponse,
    price::PricePair,
    self_ext,
};

use crate::{Contract, ContractExt, ReturnStyle};

/// Internal helpers.
impl Contract {
    pub fn price_pair(&self, oracle_response: OracleResponse) -> PricePair {
        self.configuration
            .price_oracle_configuration
            .create_price_pair(&oracle_response)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()))
    }

    pub fn execute_supply(&mut self, account_id: AccountId, amount: BorrowAssetAmount) {
        if self.supply_position_ref(account_id.clone()).is_none() {
            self.charge_for_storage(
                &account_id,
                self.storage_usage_supply_position + self.storage_usage_snapshot * 2,
            );
        }

        let snapshot = self.snapshot();
        let mut supply_position = self.get_or_create_supply_position_guard(snapshot, account_id);
        let proof = supply_position.accumulate_yield();
        supply_position.record_deposit(proof, amount, env::block_timestamp_ms());
        require!(
            supply_position.is_within_allowable_range(),
            "New supply position is outside of allowable range",
        );
    }

    pub fn execute_collateralize(
        &mut self,
        account_id: AccountId,
        amount: CollateralAssetAmount,
        price_pair: &PricePair,
    ) {
        // TODO: This creates a borrow record implicitly. If we
        // require a discrete "sign-up" step, we will need to add
        // checks before this function call.
        //
        // The sign-up step would only be NFT gating or something of
        // that sort, which is just an additional pre condition check.
        // -- https://github.com/Templar-Protocol/contract-mvp/pull/6#discussion_r1923871982
        if self.borrow_position_ref(account_id.clone()).is_none() {
            self.charge_for_storage(
                &account_id,
                self.storage_usage_borrow_position + self.storage_usage_snapshot * 2,
            );
        }

        let snapshot = self.snapshot();
        let mut borrow_position = self.get_or_create_borrow_position_guard(snapshot, account_id);
        if !borrow_position.inner().liquidation_lock.is_zero() {
            env::panic_str("Cannot add collateral while liquidation locked");
        }
        let proof = borrow_position.accumulate_interest();
        require!(
            !borrow_position
                .status(price_pair, env::block_timestamp_ms())
                .is_liquidation(),
            "Cannot add collateral when eligible for liquidation",
        );
        borrow_position.record_collateral_asset_deposit(proof, amount);
    }

    /// Returns the amount that should be returned to the account.
    pub fn execute_repay(
        &mut self,
        account_id: AccountId,
        amount: BorrowAssetAmount,
        price_pair: &PricePair,
    ) -> BorrowAssetAmount {
        let snapshot = self.snapshot();
        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id) else {
            // No borrow exists: just return the whole amount.
            return amount;
        };
        let proof = borrow_position.accumulate_interest();
        require!(
            !borrow_position
                .status(price_pair, env::block_timestamp_ms())
                .is_liquidation(),
            "Cannot repay when eligible for liquidation",
        );
        // Returns the amount that should be returned to the borrower.
        borrow_position
            .record_repay(proof, amount)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()))
    }
}

/// External helpers.
#[near]
impl Contract {
    // 3.9 Tgas
    pub const GAS_BORROW_01_CONSUME_PRICE: Gas = Gas::from_tgas(6)
        .saturating_add(FungibleAsset::<BorrowAsset>::GAS_FT_TRANSFER)
        .saturating_add(Self::GAS_BORROW_02_FINALIZE);

    #[private]
    pub fn borrow_01_consume_price(
        &mut self,
        account_id: AccountId,
        amount: BorrowAssetAmount,
        #[callback_unwrap] oracle_response: OracleResponse,
    ) -> Promise {
        let price_pair = self.price_pair(oracle_response);
        let snapshot = self.snapshot();

        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id.clone())
        else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        let interest = borrow_position.accumulate_interest();

        let initial_borrow = borrow_position
            .record_borrow_initial(
                snapshot,
                interest,
                amount,
                &price_pair,
                env::block_timestamp_ms(),
            )
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        drop(borrow_position);

        self.configuration
            .borrow_asset
            .transfer(account_id.clone(), amount)
            .then(
                self_ext!(Self::GAS_BORROW_02_FINALIZE)
                    .borrow_02_finalize(account_id, initial_borrow),
            )
    }

    // 3.1 Tgas
    pub const GAS_BORROW_02_FINALIZE: Gas = Gas::from_tgas(6);

    #[private]
    pub fn borrow_02_finalize(&mut self, account_id: AccountId, initial_borrow: InitialBorrow) {
        let snapshot = self.snapshot();
        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id) else {
            env::panic_str("Invariant violation: borrow position does not exist after transfer.");
        };

        let proof = borrow_position.accumulate_interest();
        let success = matches!(env::promise_result(0), PromiseResult::Successful(_));
        borrow_position.record_borrow_final(
            snapshot,
            proof,
            &initial_borrow,
            success,
            env::block_timestamp_ms(),
        );
    }

    // ~5.8 Tgas
    pub const GAS_EXECUTE_NEXT_SUPPLY_WITHDRAWAL_REQUEST_01_FINALIZE: Gas = Gas::from_tgas(8);

    #[private]
    pub fn execute_next_supply_withdrawal_request_01_finalize(
        &mut self,
        withdrawal_resolution: WithdrawalResolution,
        expected_success: bool,
    ) {
        asset_op!(self.borrow_asset_in_flight -= withdrawal_resolution.amount_to_account);

        // Withdrawal succeeded: remove the withdrawal request from the queue.
        // Withdrawal failed but should have succeeded: remove request but still refund.
        // Withdrawal failed: unlock the queue so they can try again.

        let withdrawal_succeeded = matches!(env::promise_result(0), PromiseResult::Successful(_));

        let snapshot = self.snapshot();
        if let Some(mut supply_position) =
            self.supply_position_guard(snapshot, withdrawal_resolution.account_id.clone())
        {
            supply_position.record_withdrawal_final(&withdrawal_resolution, withdrawal_succeeded);
        }

        if withdrawal_succeeded || expected_success {
            // TODO: If this panics, this is BIG BAD, as it means there is
            // some way to unlock the queue while a withdrawal is in-flight.
            // So, maybe we should not *actually* panic here, but do some sort of recovery?
            let (popped_account, _) = self.withdrawal_queue.try_pop().unwrap_or_else(|| {
                env::panic_str("Invariant violation: Withdrawal queue should have been locked.")
            });

            // This is another consistency check: that the account at the
            // head of the queue cannot change while transfers are
            // in-flight. This should be maintained by the queue itself.
            require!(
                popped_account == withdrawal_resolution.account_id,
                "Invariant violation: Queue shifted while locked/in-flight.",
            );
        }

        if withdrawal_succeeded {
            self.record_borrow_asset_protocol_yield(withdrawal_resolution.amount_to_fees);

            if self.cleanup_supply_position(&withdrawal_resolution.account_id) {
                self.refund_for_storage(
                    &withdrawal_resolution.account_id,
                    self.storage_usage_supply_position,
                );
            }
        } else {
            // Possible reasons for failure:
            // - MPC signer failure (multichain; TODO).
            // - The contract does not control enough of the borrow asset to
            //   fulfill the withdrawal request. That is to say, it has
            //   distributed all of the funds to current borrows.
            // - If we expected success but it still failed, this means the
            //   receiving account cannot receive tokens for some reason. For
            //   NEP-141 tokens, this usually means that the user opted out of
            //   storage management on that contract and deleted their record.

            env::log_str("The withdrawal request cannot be fulfilled at this time.");
            self.withdrawal_queue.unlock();
        }
    }

    // ~3.4 TGas
    pub const GAS_COLLATERALIZE_TRANSFER_CALL_01_CONSUME_PRICE: Gas = Gas::from_tgas(6);

    #[private]
    pub fn collateralize_transfer_call_01_consume_price(
        &mut self,
        account_id: AccountId,
        amount: CollateralAssetAmount,
        return_style: ReturnStyle,
        #[callback_unwrap] oracle_response: OracleResponse,
    ) -> serde_json::Value {
        let price_pair = self.price_pair(oracle_response);

        self.execute_collateralize(account_id, amount, &price_pair);

        return_style.serialize(CollateralAssetAmount::zero())
    }

    // ~4.3 TGas
    pub const GAS_REPAY_TRANSFER_CALL_01_CONSUME_PRICE: Gas = Gas::from_tgas(7);

    #[private]
    pub fn repay_transfer_call_01_consume_price(
        &mut self,
        account_id: AccountId,
        amount: BorrowAssetAmount,
        return_style: ReturnStyle,
        #[callback_unwrap] oracle_response: OracleResponse,
    ) -> serde_json::Value {
        let price_pair = self.price_pair(oracle_response);

        let amount = self.execute_repay(account_id, amount, &price_pair);

        return_style.serialize(amount)
    }

    // ~4.9 Tgas
    pub const GAS_LIQUIDATE_TRANSFER_CALL_01_CONSUME_PRICE: Gas = Gas::from_tgas(7)
        .saturating_add(FungibleAsset::<CollateralAsset>::GAS_FT_TRANSFER)
        .saturating_add(Self::GAS_LIQUIDATE_TRANSFER_CALL_02_FINALIZE);

    #[private]
    pub fn liquidate_transfer_call_01_consume_price(
        &mut self,
        liquidator_id: AccountId,
        amount: BorrowAssetAmount,
        msg: LiquidateMsg,
        return_style: ReturnStyle,
        #[callback_unwrap] oracle_response: OracleResponse,
    ) -> Promise {
        let price_pair = self
            .configuration
            .price_oracle_configuration
            .create_price_pair(&oracle_response)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        let result = {
            let snapshot = self.snapshot();
            let mut borrow_position = self
                .borrow_position_guard(snapshot, msg.account_id.clone())
                .unwrap_or_else(|| env::panic_str("Borrow position does not exist"));

            let proof = borrow_position.accumulate_interest();

            borrow_position
                .record_liquidation_initial(
                    proof,
                    amount,
                    msg.amount,
                    &price_pair,
                    env::block_timestamp_ms(),
                )
                .unwrap_or_else(|e| env::panic_str(&e.to_string()))
        };

        self.configuration
            .collateral_asset
            .transfer(liquidator_id.clone(), result.liquidated)
            .then(
                self_ext!(Self::GAS_LIQUIDATE_TRANSFER_CALL_02_FINALIZE)
                    .liquidate_transfer_call_02_finalize(
                        liquidator_id,
                        msg.account_id,
                        result,
                        return_style,
                    ),
            )
    }

    // ~4.6 Tgas
    pub const GAS_LIQUIDATE_TRANSFER_CALL_02_FINALIZE: Gas = Gas::from_tgas(7);

    /// Called during liquidation process; checks whether the transfer of
    /// collateral to the liquidator was successful.
    #[private]
    pub fn liquidate_transfer_call_02_finalize(
        &mut self,
        liquidator_id: AccountId,
        account_id: AccountId,
        initial_liquidation: InitialLiquidation,
        return_style: ReturnStyle,
    ) -> serde_json::Value {
        let success = matches!(env::promise_result(0), PromiseResult::Successful(_));

        let snapshot = self.snapshot();
        let mut borrow_position = self
            .borrow_position_guard(snapshot, account_id)
            .unwrap_or_else(|| {
                env::panic_str("Invariant violation: Liquidation of nonexistent position.")
            });

        if success {
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_liquidation_final(proof, liquidator_id, &initial_liquidation);
            return_style.serialize(initial_liquidation.refund)
        } else {
            // Somehow transfer of collateral failed. This could mean:
            //
            // 1. Somehow the contract does not have enough collateral
            //  available. This would be indicative of a *fundamental flaw*
            //  in the contract (i.e. this should never happen).
            //
            // 2. More likely, in a multichain context, communication
            //  broke down somewhere between the signer and the remote RPC.
            //  Could be as simple as a nonce sync issue. Should just wait
            //  and try again later.
            borrow_position.liquidation_unlock(initial_liquidation.liquidated);
            let mut return_amount = initial_liquidation.recovered;
            asset_op!(return_amount += initial_liquidation.refund);
            return_style.serialize(return_amount)
        }
    }

    // ~5.0 Tgas
    pub const GAS_WITHDRAW_COLLATERAL_01_CONSUME_PRICE: Gas = Gas::from_tgas(7)
        .saturating_add(FungibleAsset::<CollateralAsset>::GAS_FT_TRANSFER)
        .saturating_add(Self::GAS_WITHDRAW_COLLATERAL_02_FINALIZE);

    #[private]
    pub fn withdraw_collateral_01_consume_price(
        &mut self,
        account_id: AccountId,
        amount: CollateralAssetAmount,
        #[callback_unwrap] oracle_response: OracleResponse,
    ) -> Promise {
        let price_pair = self.price_pair(oracle_response);

        let snapshot = self.snapshot();
        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id.clone())
        else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        let proof = borrow_position.accumulate_interest();
        borrow_position.record_collateral_asset_withdrawal(proof, amount);

        require!(
            borrow_position
                .status(&price_pair, env::block_timestamp_ms())
                .is_healthy(),
            "Borrow position must be healthy after collateral withdrawal",
        );

        drop(borrow_position);

        self.configuration
            .collateral_asset
            .transfer(account_id.clone(), amount)
            .then(
                self_ext!(Self::GAS_WITHDRAW_COLLATERAL_02_FINALIZE)
                    .withdraw_collateral_02_finalize(account_id, amount),
            )
    }

    // ~2.2 Tgas
    pub const GAS_WITHDRAW_COLLATERAL_02_FINALIZE: Gas = Gas::from_tgas(5);

    #[private]
    pub fn withdraw_collateral_02_finalize(
        &mut self,
        account_id: AccountId,
        amount: CollateralAssetAmount,
    ) {
        let transfer_was_successful =
            matches!(env::promise_result(0), PromiseResult::Successful(_));

        if transfer_was_successful {
            if self.cleanup_borrow_position(&account_id) {
                self.refund_for_storage(&account_id, self.storage_usage_borrow_position);
            }
        } else {
            let snapshot = self.snapshot();
            let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id) else {
                env::panic_str(
                    "Invariant violation: Borrow position must exist after collateral withdrawal.",
                );
            };

            let proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_deposit(proof, amount);
        }
    }

    // ~2.1 Tgas
    pub const GAS_WITHDRAW_STATIC_YIELD_01_FINALIZE: Gas = Gas::from_tgas(5);

    #[private]
    pub fn withdraw_static_yield_01_finalize(
        &mut self,
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
        collateral_asset_amount: CollateralAssetAmount,
    ) {
        let mut static_yield = self.static_yield.get(&account_id).unwrap_or_else(|| {
            env::panic_str("Invariant violation: static yield entry must exist during callback")
        });
        let mut i = 0;

        if !borrow_asset_amount.is_zero() {
            if matches!(env::promise_result(i), PromiseResult::Failed) {
                asset_op!(static_yield.borrow_asset += borrow_asset_amount);
            }
            i += 1;
        }

        if !collateral_asset_amount.is_zero()
            && matches!(env::promise_result(i), PromiseResult::Failed)
        {
            asset_op!(static_yield.collateral_asset += collateral_asset_amount);
        }
    }
}
