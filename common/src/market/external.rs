use std::collections::HashMap;

use near_sdk::{near, AccountId, Promise, PromiseOrValue};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    number::Decimal,
    oracle::pyth::OracleResponse,
    snapshot::Snapshot,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use super::{BorrowAssetMetrics, MarketConfiguration};

#[derive(Debug, Clone, Copy, Default)]
#[near(serializers = [json, borsh])]
pub enum HarvestYieldMode {
    #[default]
    Default,
    Compounding,
    SnapshotLimit(u32),
}

#[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    /// Retrieve the market configuration.
    fn get_configuration(&self) -> MarketConfiguration;

    /// Retrieve the current snapshot (in progress; not yet finalized).
    fn get_current_snapshot(&self) -> &Snapshot;

    /// Retrieve the count of finalized snapshots.
    fn get_finalized_snapshots_len(&self) -> u32;

    /// Retrieve a list of finalized snapshots.
    fn list_finalized_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot>;

    /// Retrieve current contract metrics about borrow asset deposit,
    /// availability, usage, etc.
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;

    // ==================
    // BORROW FUNCTIONS
    // ==================

    // *_on_transfer where msg = Collateralize
    // *_on_transfer where msg = Repay

    /// Retrieve a map of borrow positions.
    fn list_borrow_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, BorrowPosition>;

    /// Retrieve a specific borrow position, with estimated fees.
    ///
    /// This function may report fees slightly inaccurately. This is because
    /// the function has to estimate what fees will be applied between the last
    /// finalized market snapshot and the (present) time when the function was
    /// called.
    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;

    /// Retrieves the status of a borrow position (healthy or in liquidation)
    /// given some asset prices.
    ///
    /// This is just a read-only function, so it does not validate the price
    /// data. It is intended to be called by liquidators so they can easily see
    /// whether a position is eligible for liquidation.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus>;

    /// Transfers `amount` of borrow asset tokens to the caller, provided
    /// their borrow position will still be sufficiently collateralized.
    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise;

    /// Transfers `amount` of collateral asset tokens to the caller, provided
    /// their borrow position will still be sufficiently collateralized.
    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise;

    /// Applies interest to the predecessor's borrow record.
    /// Not likely to be used in real life, since there it does not affect the
    /// final interest calculation, and rounds fractional interest UP.
    fn apply_interest(&mut self, account_id: Option<AccountId>, snapshot_limit: Option<u32>);

    // ================
    // SUPPLY FUNCTIONS
    // ================

    // *_on_transfer where msg = Supply

    /// Retrieves a set of supply positions.
    fn list_supply_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, SupplyPosition>;

    /// Retrieves a supply position record, with estimated yield.
    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition>;

    /// Enters a supply position into the withdrawal queue, requesting to
    /// withdraw `amount` borrow asset tokens.
    ///
    /// If the account is already in the queue, it will be moved to the back
    /// of the queue with the updated amount.
    fn create_supply_withdrawal_request(&mut self, amount: BorrowAssetAmount);

    /// Removes a supply position from the withdrawal queue.
    fn cancel_supply_withdrawal_request(&mut self);

    /// Attempts to satisfy the first withdrawal request in the queue.
    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()>;

    /// Retrieves the status of a withdrawal request in the queue.
    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus>;

    /// Retrieves the status of the withdrawal queue.
    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus;

    /// Claim any distributed yield to the supply record.
    /// If mode is set to [`HarvestYieldMode::Compounding`], the all of the
    /// yield (including any harvested in previous, non-compounding
    /// `harvest_yield` calls) will be deposited to the supply record, so it
    /// can be withdrawn and will contribute to future yield calculations.
    fn harvest_yield(
        &mut self,
        account_id: Option<AccountId>,
        mode: Option<HarvestYieldMode>,
    ) -> BorrowAssetAmount;

    /// This value is an *expected average over time*.
    /// Supply positions actually earn all of their yield the instant it is
    /// distributed.
    fn get_last_yield_rate(&self) -> Decimal;

    // =====================
    // LIQUIDATION FUNCTIONS
    // =====================

    // *_on_transfer where msg = Liquidate { account_id }

    // =================
    // YIELD FUNCTIONS
    // =================

    /// Retrieves the amount of yield earned by an account statically
    /// configured to earn yield (e.g. [`MarketConfiguration::yield_weights`]
    /// or [`MarketConfiguration::protocol_account_id`]).
    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord>;

    /// Attempts to withdraw the amount of yield earned by an account
    /// statically configured to earn yield (e.g.
    /// [`MarketConfiguration::yield_weights`] or
    /// [`MarketConfiguration::protocol_account_id`]).
    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise;
}
