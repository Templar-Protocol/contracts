use std::collections::HashMap;

use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{BorrowAssetMetrics, HarvestYieldMode, MarketConfiguration},
    oracle::pyth::OracleResponse,
    snapshot::Snapshot,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
    Decimal,
};
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::{common::Pagination, primitive::PublicKey, NearToken};

/// Get market configuration.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getConfiguration", output = MarketConfiguration)]
pub struct GetConfiguration {
    pub market_id: AccountId,
}

/// List borrow positions.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.listBorrowPositions", output = ListBorrowPositionsResult)]
pub struct ListBorrowPositions {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsResult {
    pub positions: HashMap<AccountId, BorrowPosition>,
}

/// Get the current market snapshot.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getCurrentSnapshot", output = Snapshot)]
pub struct GetCurrentSnapshot {
    pub market_id: AccountId,
}

/// Get finalized snapshot count.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getFinalizedSnapshotsLen", output = u32)]
pub struct GetFinalizedSnapshotsLen {
    pub market_id: AccountId,
}

/// List finalized snapshots.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.listFinalizedSnapshots", output = ListFinalizedSnapshotsResult)]
pub struct ListFinalizedSnapshots {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFinalizedSnapshotsResult {
    pub snapshots: Vec<Snapshot>,
}

/// Get borrow asset metrics.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getBorrowAssetMetrics", output = BorrowAssetMetrics)]
pub struct GetBorrowAssetMetrics {
    pub market_id: AccountId,
}

/// Get a borrow position.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getBorrowPosition", output = GetBorrowPositionResult)]
pub struct GetBorrowPosition {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionResult {
    pub position: Option<BorrowPosition>,
}

/// Get pending borrow interest.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getBorrowPositionPendingInterest", output = GetBorrowPositionPendingInterestResult)]
pub struct GetBorrowPositionPendingInterest {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionPendingInterestResult {
    pub amount: Option<BorrowAssetAmount>,
}

/// Get borrow status for an account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getBorrowStatus", output = GetBorrowStatusResult)]
pub struct GetBorrowStatus {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub oracle_response: OracleResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowStatusResult {
    pub status: Option<BorrowStatus>,
}

/// List supply positions.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.listSupplyPositions", output = ListSupplyPositionsResult)]
pub struct ListSupplyPositions {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListSupplyPositionsResult {
    pub positions: HashMap<AccountId, SupplyPosition>,
}

/// Get a supply position.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getSupplyPosition", output = GetSupplyPositionResult)]
pub struct GetSupplyPosition {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionResult {
    pub position: Option<SupplyPosition>,
}

/// Get pending supply yield.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getSupplyPositionPendingYield", output = GetSupplyPositionPendingYieldResult)]
pub struct GetSupplyPositionPendingYield {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionPendingYieldResult {
    pub amount: Option<BorrowAssetAmount>,
}

/// Get supply withdrawal request status.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getSupplyWithdrawalRequestStatus", output = GetSupplyWithdrawalRequestStatusResult)]
pub struct GetSupplyWithdrawalRequestStatus {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalRequestStatusResult {
    pub status: Option<WithdrawalRequestStatus>,
}

/// Get supply withdrawal queue status.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getSupplyWithdrawalQueueStatus", output = WithdrawalQueueStatus)]
pub struct GetSupplyWithdrawalQueueStatus {
    pub market_id: AccountId,
}

/// Get the last yield rate.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getLastYieldRate", output = Decimal)]
pub struct GetLastYieldRate {
    pub market_id: AccountId,
}

/// Get accumulated static yield.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "market.getStaticYield", output = GetStaticYieldResult)]
pub struct GetStaticYield {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

/// A market account's static yield, across market versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaticYield {
    /// Markets >= 1.1.0 expose a yield accumulator.
    Accumulator {
        accumulator: Accumulator<BorrowAsset>,
    },
    /// Pre-1.1.0 markets report only the borrow-denominated total.
    Legacy {
        borrow_asset_total: BorrowAssetAmount,
    },
}

impl StaticYield {
    /// Total static yield denominated in the borrow asset.
    #[must_use]
    pub fn borrow_asset_total(&self) -> BorrowAssetAmount {
        match self {
            Self::Accumulator { accumulator } => accumulator.get_total(),
            Self::Legacy { borrow_asset_total } => *borrow_asset_total,
        }
    }

    /// The yield accumulator, if this market exposes one.
    #[must_use]
    pub fn accumulator(&self) -> Option<&Accumulator<BorrowAsset>> {
        match self {
            Self::Accumulator { accumulator } => Some(accumulator),
            Self::Legacy { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStaticYieldResult {
    /// The account's static yield record, or `None` if it has none.
    pub record: Option<StaticYield>,
}

impl GetStaticYieldResult {
    /// Total accumulated static yield denominated in the borrow asset (zero if
    /// the account has no record).
    #[must_use]
    pub fn borrow_asset_total(&self) -> BorrowAssetAmount {
        self.record
            .as_ref()
            .map_or_else(BorrowAssetAmount::zero, StaticYield::borrow_asset_total)
    }
}

/// Borrow from a market.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.borrow")]
pub struct Borrow {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}

/// Create a market from the registry.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.create")]
pub struct Create {
    pub registry_id: AccountId,
    pub name: String,
    pub version_key: String,
    pub configuration: MarketConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}

/// Supply assets to a market.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.supply")]
pub struct Supply {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}

/// Deposit collateral into a market.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.collateralize")]
pub struct Collateralize {
    pub market_id: AccountId,
    pub amount: CollateralAssetAmount,
}

/// Withdraw collateral from a market.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.withdrawCollateral")]
pub struct WithdrawCollateral {
    pub market_id: AccountId,
    pub amount: CollateralAssetAmount,
}

/// Apply interest to a market account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.applyInterest")]
pub struct ApplyInterest {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub snapshot_limit: Option<u32>,
}

/// Repay borrowed assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.repay")]
pub struct Repay {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
    pub account_id: Option<AccountId>,
}

/// Create a supply withdrawal request.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.createSupplyWithdrawalRequest")]
pub struct CreateSupplyWithdrawalRequest {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}

/// Cancel a supply withdrawal request.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.cancelSupplyWithdrawalRequest")]
pub struct CancelSupplyWithdrawalRequest {
    pub market_id: AccountId,
}

/// Execute the next supply withdrawal request.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.executeNextSupplyWithdrawalRequest")]
pub struct ExecuteNextSupplyWithdrawalRequest {
    pub market_id: AccountId,
    pub batch_limit: Option<u32>,
}

/// Withdraw supplied assets.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.withdrawSupply")]
pub struct WithdrawSupply {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
    pub batch_limit: Option<u32>,
}

/// Liquidate an unhealthy account.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.liquidate")]
pub struct Liquidate {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub liquidation_amount: BorrowAssetAmount,
    pub collateral_amount: Option<CollateralAssetAmount>,
}

/// Harvest market yield.
#[derive(MethodSpec, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.harvestYield")]
pub struct HarvestYield {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub mode: Option<HarvestYieldMode>,
}

/// Accumulate static yield.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.accumulateStaticYield")]
pub struct AccumulateStaticYield {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub snapshot_limit: Option<u32>,
}

/// Withdraw static yield.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "market.withdrawStaticYield")]
pub struct WithdrawStaticYield {
    pub market_id: AccountId,
    pub amount: Option<BorrowAssetAmount>,
}
