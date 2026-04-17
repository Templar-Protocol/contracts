use std::collections::HashMap;

use blockchain_gateway_core::common::Pagination;
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{BorrowAssetMetrics, HarvestYieldMode, MarketConfiguration},
    number::Decimal,
    oracle::pyth::OracleResponse,
    snapshot::Snapshot,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use crate::client::{
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

#[derive(serde::Serialize)]
pub struct GetBorrowPositionPendingInterestArgs {
    pub account_id: near_account_id::AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct AccountIdArg {
    pub account_id: near_account_id::AccountId,
}

#[derive(serde::Serialize)]
pub struct GetBorrowStatusArgs {
    pub account_id: near_account_id::AccountId,
    pub oracle_response: OracleResponse,
}

#[derive(serde::Serialize)]
pub struct GetSupplyPositionPendingYieldArgs {
    pub account_id: near_account_id::AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct ApplyInterestArgs {
    pub account_id: Option<near_account_id::AccountId>,
    pub snapshot_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct AmountArg<T> {
    pub amount: T,
}

#[derive(serde::Serialize)]
pub struct BatchLimitArg {
    pub batch_limit: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct HarvestYieldArgs {
    pub account_id: Option<near_account_id::AccountId>,
    pub mode: Option<HarvestYieldMode>,
}

#[derive(serde::Serialize)]
pub struct AccumulateStaticYieldArgs {
    pub account_id: Option<near_account_id::AccountId>,
    pub snapshot_limit: Option<u32>,
}

#[derive(Clone)]
pub struct MarketClient<'a> {
    pub(crate) inner: &'a NearClient,
    pub(crate) contract_id: blockchain_gateway_core::MarketId,
}

impl BoundContractClient for MarketClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id.0
    }
}

impl MarketClient<'_> {
    contract_views! {
        pub fn get_configuration(()) -> MarketConfiguration;
        pub fn list_borrow_positions(Pagination) -> HashMap<near_account_id::AccountId, BorrowPosition>;
        pub fn get_current_snapshot(()) -> Snapshot;
        pub fn get_finalized_snapshots_len(()) -> u32;
        pub fn list_finalized_snapshots(Pagination) -> Vec<Snapshot>;
        pub fn get_borrow_asset_metrics(()) -> BorrowAssetMetrics;
        pub fn get_borrow_position(AccountIdArg) -> Option<BorrowPosition>;
        pub fn get_borrow_position_pending_interest(GetBorrowPositionPendingInterestArgs) -> Option<BorrowAssetAmount>;
        pub fn get_borrow_status(GetBorrowStatusArgs) -> Option<BorrowStatus>;
        pub fn list_supply_positions(Pagination) -> HashMap<near_account_id::AccountId, SupplyPosition>;
        pub fn get_supply_position(AccountIdArg) -> Option<SupplyPosition>;
        pub fn get_supply_position_pending_yield(GetSupplyPositionPendingYieldArgs) -> Option<BorrowAssetAmount>;
        pub fn get_supply_withdrawal_request_status(AccountIdArg) -> Option<WithdrawalRequestStatus>;
        pub fn get_supply_withdrawal_queue_status(()) -> WithdrawalQueueStatus;
        pub fn get_last_yield_rate(()) -> Decimal;
        pub fn get_static_yield(AccountIdArg) -> Option<Accumulator<BorrowAsset>>;
    }

    contract_writes! {
        pub(crate) fn borrow(AmountArg<BorrowAssetAmount>);
        pub(crate) fn withdraw_collateral(AmountArg<CollateralAssetAmount>);
        pub(crate) fn apply_interest(ApplyInterestArgs);
        pub(crate) fn create_supply_withdrawal_request(AmountArg<BorrowAssetAmount>);
        pub(crate) fn cancel_supply_withdrawal_request(());
        pub(crate) fn execute_next_supply_withdrawal_request(BatchLimitArg);
        pub(crate) fn harvest_yield(HarvestYieldArgs);
        pub(crate) fn accumulate_static_yield(AccumulateStaticYieldArgs);
        pub(crate) fn withdraw_static_yield(AmountArg<Option<BorrowAssetAmount>>);
    }
}
