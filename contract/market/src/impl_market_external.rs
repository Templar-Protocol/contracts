use near_sdk::{env, near, require, AccountId, Promise, PromiseOrValue};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{BorrowAssetMetrics, MarketConfiguration, MarketExternalInterface},
    number::Decimal,
    oracle::pyth::OracleResponse,
    self_ext,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use crate::{Contract, ContractExt};

#[near]
impl MarketExternalInterface for Contract {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_snapshots_len(&self) -> u32 {
        self.snapshots.len()
    }

    fn list_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot> {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        self.snapshots
            .iter()
            .skip(offset)
            .take(count)
            .collect::<Vec<_>>()
    }

    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics {
        BorrowAssetMetrics {
            available: self.get_borrow_asset_available_to_borrow(),
            deposited: self.borrow_asset_deposited,
            borrowed: self.borrow_asset_borrowed,
        }
    }

    fn list_borrows(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        self.iter_borrow_account_ids()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn list_supplys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        self.iter_supply_account_ids()
            .skip(offset)
            .take(count)
            .collect()
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
        let borrow_position = self.get_borrow_position(account_id)?;

        let price_pair = self
            .configuration
            .balance_oracle
            .create_price_pair(&oracle_response)
            .unwrap_or_else(|e| env::panic_str(&e.to_string()));

        Some(self.configuration.borrow_status(
            &borrow_position,
            &price_pair,
            env::block_timestamp_ms(),
        ))
    }

    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise {
        require!(!amount.is_zero(), "Borrow amount must be greater than zero");
        require!(
            amount >= self.configuration.borrow_minimum_amount,
            "Borrow amount is smaller than minimum allowed",
        );
        require!(
            amount <= self.configuration.borrow_maximum_amount,
            "Borrow amount is greater than maximum allowed",
        );

        let account_id = env::predecessor_account_id();

        self.configuration
            .balance_oracle
            .retrieve_price_pair()
            .then(self_ext!().borrow_01_consume_price(account_id, amount))
    }

    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise {
        let account_id = env::predecessor_account_id();

        let Some(mut borrow_position) = self.borrow_position_guard(account_id.clone()) else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        if borrow_position
            .inner()
            .get_total_borrow_asset_liability()
            .is_zero()
        {
            // No need to retrieve prices, since there is zero liability.
            borrow_position.record_collateral_asset_withdrawal(amount);
            drop(borrow_position);

            self.configuration
                .collateral_asset
                .transfer(account_id.clone(), amount)
                .then(self_ext!().withdraw_collateral_02_finalize(account_id, amount))
        } else {
            drop(borrow_position);
            // They still have liability, so we need to check prices.
            self.configuration
                .balance_oracle
                .retrieve_price_pair()
                .then(self_ext!().withdraw_collateral_01_consume_price(account_id, amount))
        }
    }

    fn apply_interest(&mut self, snapshot_limit: Option<u32>) {
        let predecessor = env::predecessor_account_id();
        if let Some(mut borrow_position) = self.borrow_position_guard(predecessor) {
            borrow_position.accumulate_interest_partial(snapshot_limit.unwrap_or(u32::MAX));
        }
    }

    fn get_last_interest_rate(&self) -> Decimal {
        self.get_interest_rate_for_snapshot(self.get_last_snapshot())
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
        let Some(supply_position) =
            self.supply_position_ref(predecessor.clone())
                .filter(|supply_position| {
                    !supply_position.inner().get_borrow_asset_deposit().is_zero()
                })
        else {
            env::panic_str("Supply position does not exist");
        };

        // We do check here, as well as during the execution.
        // This check really only ensures that the `depth` reported by
        // get_supply_withdrawal_queue_status() is realistically accurate.
        require!(
            supply_position.inner().get_borrow_asset_deposit() >= amount,
            "Attempt to withdraw more than current deposit",
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

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(
                    withdrawal_resolution.account_id.clone(),
                    withdrawal_resolution.amount_to_account,
                )
                .then(
                    self_ext!()
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
        compounding: Option<bool>,
        snapshot_limit: Option<u32>,
    ) -> BorrowAssetAmount {
        let predecessor = env::predecessor_account_id();
        let Some(mut supply_position) = self.supply_position_guard(predecessor) else {
            return BorrowAssetAmount::zero();
        };

        match (compounding.unwrap_or(false), snapshot_limit) {
            (true, Some(_)) => env::panic_str("`compounding` and `snapshot_limit` are exclusive"),
            (true, None) => {
                let proof = supply_position.accumulate_yield();
                // Compound yield by withdrawing it and recording it as an immediate deposit.
                let total_yield = supply_position.inner().borrow_asset_yield.get_total();
                supply_position.record_yield_withdrawal(total_yield);
                supply_position.record_deposit(proof, total_yield, env::block_timestamp_ms());
                return total_yield;
            }
            (false, Some(snapshot_limit)) => {
                supply_position.accumulate_yield_partial(snapshot_limit);
            }
            _ => {
                supply_position.accumulate_yield();
            }
        }

        BorrowAssetAmount::zero()
    }

    fn get_last_yield_rate(&self) -> Decimal {
        let last_snapshot = self.get_last_snapshot();
        let last_interest_rate = self.get_interest_rate_for_snapshot(last_snapshot);
        let deposited: Decimal = last_snapshot.deposited.into();
        if deposited.is_zero() {
            return Decimal::ZERO;
        }
        let borrowed: Decimal = last_snapshot.borrowed.into();
        let supply_weight: Decimal = self.configuration.yield_weights.supply.get().into();
        let total_weight: Decimal = self.configuration.yield_weights.total_weight().get().into();

        last_interest_rate * borrowed * supply_weight / deposited / total_weight
    }

    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord> {
        self.static_yield.get(&account_id)
    }

    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise {
        let predecessor = env::predecessor_account_id();
        let Some(mut static_yield_record) = self.static_yield.get(&predecessor) else {
            env::panic_str("Yield record does not exist");
        };

        let (borrow_asset_amount, collateral_asset_amount) =
            if borrow_asset_amount.is_none() && collateral_asset_amount.is_none() {
                // no arguments = withdraw all
                (
                    static_yield_record.borrow_asset,
                    static_yield_record.collateral_asset,
                )
            } else {
                (
                    borrow_asset_amount.unwrap_or_default(),
                    collateral_asset_amount.unwrap_or_default(),
                )
            };

        static_yield_record
            .borrow_asset
            .split(borrow_asset_amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset yield underflow"));
        static_yield_record
            .collateral_asset
            .split(collateral_asset_amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset yield underflow"));

        self.static_yield.insert(&predecessor, &static_yield_record);

        let borrow_promise = if borrow_asset_amount.is_zero() {
            None
        } else {
            Some(
                self.configuration
                    .borrow_asset
                    .transfer(predecessor.clone(), borrow_asset_amount),
            )
        };

        let collateral_promise = if collateral_asset_amount.is_zero() {
            None
        } else {
            Some(
                self.configuration
                    .collateral_asset
                    .transfer(predecessor.clone(), collateral_asset_amount),
            )
        };

        match (borrow_promise, collateral_promise) {
            (Some(b), Some(c)) => b.and(c),
            (Some(p), _) | (_, Some(p)) => p,
            _ => env::panic_str("No yield to withdraw"),
        }
        .then(
            Self::ext(env::current_account_id()).withdraw_static_yield_01_finalize(
                predecessor,
                borrow_asset_amount,
                collateral_asset_amount,
            ),
        )
    }
}
