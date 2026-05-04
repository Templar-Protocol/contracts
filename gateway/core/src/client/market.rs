use std::collections::HashMap;

use moka::sync::Cache;
use near_account_id::AccountId;
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
use templar_gateway_types::common::Pagination;

use crate::client::{
    cache::{immutable_cache, load_cached},
    macros::{contract_views, contract_writes},
    NearClient,
};

use super::BoundContractClient;

const MARKET_CONFIGURATION_CACHE_CAPACITY: u64 = 256;

#[derive(Clone)]
pub(crate) struct MarketClientCaches {
    pub configuration: Cache<AccountId, std::sync::Arc<MarketConfiguration>>,
}

impl MarketClientCaches {
    pub fn new() -> Self {
        Self {
            configuration: immutable_cache(MARKET_CONFIGURATION_CACHE_CAPACITY),
        }
    }
}

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
    pub(crate) contract_id: AccountId,
}

impl BoundContractClient for MarketClient<'_> {
    fn client(&self) -> &NearClient {
        self.inner
    }

    fn contract_id(&self) -> &near_account_id::AccountIdRef {
        &self.contract_id
    }
}

impl MarketClient<'_> {
    pub async fn cached_get_configuration(&self) -> crate::GatewayResult<MarketConfiguration> {
        load_cached(
            &self.inner.cache().market.configuration,
            self.contract_id.clone(),
            {
                let near = self.inner.clone();
                let contract_id = self.contract_id.clone();
                move || async move { near.market(contract_id).get_configuration(()).await }
            },
        )
        .await
    }

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
        pub fn borrow(AmountArg<BorrowAssetAmount>);
        pub fn withdraw_collateral(AmountArg<CollateralAssetAmount>);
        pub fn apply_interest(ApplyInterestArgs);
        pub fn create_supply_withdrawal_request(AmountArg<BorrowAssetAmount>);
        pub fn cancel_supply_withdrawal_request(());
        pub fn execute_next_supply_withdrawal_request(BatchLimitArg);
        pub fn harvest_yield(HarvestYieldArgs);
        pub fn accumulate_static_yield(AccumulateStaticYieldArgs);
        pub fn withdraw_static_yield(AmountArg<Option<BorrowAssetAmount>>);
    }
}
