use std::collections::HashMap;

use near_sdk::{env, near, require, AccountId, Promise, PromiseOrValue};
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    asset_op,
    borrow::{BorrowPosition, BorrowStatus},
    contract::list,
    market::{BorrowAssetMetrics, HarvestYieldMode, MarketConfiguration, MarketExternalInterface},
    number::Decimal,
    oracle::pyth::OracleResponse,
    self_ext,
    snapshot::Snapshot,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use crate::{Contract, ContractExt};

#[near]
impl MarketExternalInterface for Contract {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_current_snapshot(&self) -> Snapshot {
        self.current_snapshot()
    }

    fn get_finalized_snapshots_len(&self) -> u32 {
        self.finalized_snapshots.len()
    }

    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics {
        BorrowAssetMetrics {
            available: self.get_borrow_asset_available_to_borrow(),
            deposited_active: self.borrow_asset_deposited_active,
            deposited_incoming: self
                .borrow_asset_deposited_incoming
                .iter()
                .map(|incoming| (incoming.activate_at_snapshot_index, incoming.amount))
                .collect(),
            borrowed: self.borrowed(),
        }
    }

    fn list_borrow_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, BorrowPosition> {
        list(self.iter_borrow_positions(), offset, count)
    }

    fn list_finalized_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot> {
        list(&self.finalized_snapshots, offset, count)
    }

    fn list_supply_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, SupplyPosition> {
        list(self.iter_supply_positions(), offset, count)
    }

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition> {
        let mut borrow_position = self.borrow_position_ref(account_id)?;
        borrow_position.with_pending_interest();
        Some(borrow_position.inner().clone())
    }

    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus> {
        let borrow_position = self.borrow_position_ref(account_id)?;

        let price_pair = self
            .configuration
            .price_oracle_configuration
            .create_price_pair(&oracle_response)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        Some(borrow_position.status(&price_pair, env::block_timestamp_ms()))
    }

    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise {
        require!(!amount.is_zero(), "Borrow amount must be greater than zero");
        let account_id = env::predecessor_account_id();
        require!(
            self.borrow_position_ref(account_id.clone()).is_some(),
            "Borrow position does not exist",
        );

        self.configuration
            .price_oracle_configuration
            .retrieve_price_pair()
            .then(
                self_ext!(Self::GAS_BORROW_01_CONSUME_PRICE)
                    .borrow_01_consume_price(account_id, amount),
            )
    }

    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise {
        let account_id = env::predecessor_account_id();

        let snapshot = self.snapshot();
        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id.clone())
        else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        if borrow_position
            .inner()
            .get_total_borrow_asset_liability()
            .is_zero()
        {
            // No need to retrieve prices, since there is zero liability.
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_withdrawal(proof, amount);
            drop(borrow_position);

            self.configuration
                .collateral_asset
                .transfer(account_id.clone(), amount)
                .then(
                    self_ext!(Self::GAS_WITHDRAW_COLLATERAL_02_FINALIZE)
                        .withdraw_collateral_02_finalize(account_id, amount),
                )
        } else {
            drop(borrow_position);
            // They still have liability, so we need to check prices.
            self.configuration
                .price_oracle_configuration
                .retrieve_price_pair()
                .then(
                    self_ext!(Self::GAS_WITHDRAW_COLLATERAL_01_CONSUME_PRICE)
                        .withdraw_collateral_01_consume_price(account_id, amount),
                )
        }
    }

    fn apply_interest(&mut self, account_id: Option<AccountId>, snapshot_limit: Option<u32>) {
        let account_id = account_id.unwrap_or_else(env::predecessor_account_id);
        let snapshot = self.snapshot();
        if let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id) {
            borrow_position.accumulate_interest_partial(snapshot_limit.unwrap_or(u32::MAX));
        }
    }

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition> {
        let mut supply_position = self.supply_position_ref(account_id)?;
        supply_position.with_pending_yield_estimate();
        Some(supply_position.inner().clone())
    }

    /// If the predecessor has already entered the queue, calling this function
    /// will reset the position to the back of the queue.
    fn create_supply_withdrawal_request(&mut self, amount: BorrowAssetAmount) {
        require!(
            !amount.is_zero(),
            "Amount to withdraw must be greater than zero",
        );
        let predecessor = env::predecessor_account_id();
        let Some(supply_position) = self
            .supply_position_ref(predecessor.clone())
            .filter(|supply_position| !supply_position.total_deposit().is_zero())
        else {
            env::panic_str("Supply position does not exist");
        };

        // We do check here, as well as during the execution.
        // This check really only ensures that the `depth` reported by
        // get_supply_withdrawal_queue_status() is realistically accurate.
        require!(
            supply_position.total_deposit() >= amount,
            "Attempt to withdraw more than current deposit",
        );
        require!(
            self.configuration.supply_withdrawal_range.contains(amount),
            "Withdrawal amount is outside of allowable range",
        );

        self.withdrawal_queue.remove(&predecessor);
        self.withdrawal_queue.insert_or_update(&predecessor, amount);
    }

    fn cancel_supply_withdrawal_request(&mut self) {
        self.withdrawal_queue.remove(&env::predecessor_account_id());
    }

    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        let Some(withdrawal_resolution) = self
            .try_lock_next_withdrawal_request()
            .unwrap_or_else(|e| env::panic_str(&e.to_string()))
        else {
            env::log_str("Supply position does not exist: skipping.");
            return PromiseOrValue::Value(());
        };

        // There may be loose/untracked funds that the contract controls but
        // does not account for in internal accounting.
        let has_sufficient_liquidity = u128::from(self.borrow_asset_deposited_active)
            .saturating_add(u128::from(self.total_incoming()))
            .checked_sub(u128::from(self.borrowed()))
            .is_some();

        require!(
            has_sufficient_liquidity,
            "Insufficient liquidity to fulfill the request at this time",
        );

        asset_op!(
            self.borrow_asset_withdrawal_in_flight += withdrawal_resolution.amount_to_account
        );

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(
                    withdrawal_resolution.account_id.clone(),
                    withdrawal_resolution.amount_to_account,
                )
                .then(
                    self_ext!(Self::GAS_EXECUTE_NEXT_SUPPLY_WITHDRAWAL_REQUEST_01_FINALIZE)
                        .execute_next_supply_withdrawal_request_01_finalize(withdrawal_resolution),
                ),
        )
    }

    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus> {
        self.withdrawal_queue.get_request_status(&account_id)
    }

    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus {
        self.withdrawal_queue.get_status()
    }

    fn harvest_yield(
        &mut self,
        account_id: Option<AccountId>,
        mode: Option<HarvestYieldMode>,
    ) -> BorrowAssetAmount {
        let mode = mode.unwrap_or_default();
        let predecessor = env::predecessor_account_id();
        let account_id = account_id.unwrap_or_else(|| predecessor.clone());

        require!(
            account_id == predecessor || !matches!(mode, HarvestYieldMode::Compounding),
            "Only the position holder can compound yield",
        );

        let snapshot = self.snapshot();
        let Some(mut supply_position) = self.supply_position_guard(snapshot, account_id) else {
            return BorrowAssetAmount::zero();
        };

        match mode {
            HarvestYieldMode::Compounding => {
                let proof = supply_position.accumulate_yield();
                // Compound yield by withdrawing it and recording it as an immediate deposit.
                let total_yield = supply_position.total_yield();
                supply_position.record_yield_withdrawal(total_yield);
                supply_position.record_deposit(proof, total_yield, env::block_timestamp_ms());
                require!(
                    supply_position.is_within_allowable_range(),
                    "New supply position is outside of allowable range",
                );
                return total_yield;
            }
            HarvestYieldMode::SnapshotLimit(limit) => {
                supply_position.accumulate_yield_partial(limit);
            }
            HarvestYieldMode::Default => {
                supply_position.accumulate_yield();
            }
        }

        BorrowAssetAmount::zero()
    }

    fn get_last_yield_rate(&self) -> Decimal {
        self.configuration.yield_rate(&self.current_snapshot())
    }

    fn get_static_yield(&self, account_id: AccountId) -> Option<Accumulator<BorrowAsset>> {
        self.static_yield.get(&account_id)
    }

    fn accumulate_static_yield(
        &mut self,
        account_id: Option<AccountId>,
        snapshot_limit: Option<u32>,
    ) {
        self.market
            .accumulate_static_yield(
                &account_id.unwrap_or_else(env::predecessor_account_id),
                snapshot_limit.unwrap_or(u32::MAX),
            )
            .unwrap_or_else(|_| env::panic_str("This account does not earn static yield"));
    }

    fn withdraw_static_yield(&mut self, amount: Option<BorrowAssetAmount>) -> Promise {
        let predecessor = env::predecessor_account_id();
        let Some(mut yield_record) = self.static_yield.get(&predecessor) else {
            env::panic_str("Yield record does not exist");
        };

        let amount = amount.unwrap_or_else(|| yield_record.get_total());

        yield_record
            .remove(amount)
            .unwrap_or_else(|| env::panic_str("Attempt to overdraw"));

        self.static_yield.insert(&predecessor, &yield_record);

        self.configuration
            .borrow_asset
            .transfer(predecessor.clone(), amount)
            .then(
                self_ext!(Self::GAS_WITHDRAW_STATIC_YIELD_01_FINALIZE)
                    .withdraw_static_yield_01_finalize(predecessor, amount),
            )
    }
}
