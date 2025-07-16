use std::collections::HashMap;

use near_sdk::{near, AccountId, Promise, PromiseOrValue};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount, FungibleAsset, IncentiveAsset},
    borrow::{BorrowPosition, BorrowStatus},
    incentive::Incentive,
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

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_current_snapshot(&self) -> &Snapshot;
    fn get_finalized_snapshots_len(&self) -> u32;
    fn list_finalized_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot>;
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;

    // ================
    // BORROW FUNCTIONS
    // ================

    // ft_on_receive :: where msg = Collateralize
    // ft_on_receive :: where msg = Repay

    fn list_borrow_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, BorrowPosition>;
    /// This function may report fees slightly inaccurately. This is because
    /// the function has to estimate what fees will be applied between the last
    /// market snapshot and the (present) time when the function was called.
    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus>;

    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise;
    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise;

    /// Applies interest to the predecessor's borrow record.
    /// Not likely to be used in real life, since there it does not affect the
    /// final interest calculation, and rounds fractional interest UP.
    fn apply_interest(&mut self, account_id: Option<AccountId>, snapshot_limit: Option<u32>);

    // ================
    // SUPPLY FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support supplying of remote assets.

    // ft_on_receive :: where msg = Supply

    fn list_supply_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, SupplyPosition>;
    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition>;

    fn create_supply_withdrawal_request(&mut self, amount: BorrowAssetAmount);
    fn cancel_supply_withdrawal_request(&mut self);
    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()>;
    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus>;
    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus;

    /// Claim any distributed yield to the supply record.
    /// If mode is set to `compounding`, the all of the yield (including any
    /// harvested in previous, non-compounding `harvest_yield` calls) is
    /// deposited to the supply record, so it will contribute to future yield
    /// calculations.
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

    // ft_on_receive :: where msg = Liquidate { account_id }

    // ===============
    // YIELD FUNCTIONS
    // ===============
    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord>;
    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise;

    // ==========
    // INCENTIVES
    // ==========

    // on receive token where msg = CreateIncentive
    fn list_incentives(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<FungibleAsset<IncentiveAsset>, Incentive>;
}
