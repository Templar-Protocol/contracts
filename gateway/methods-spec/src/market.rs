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
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_gateway_types::{common::Pagination, primitive::PublicKey, NearToken};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigurationParams {
    pub market_id: AccountId,
}

pub type GetConfigurationResult = MarketConfiguration;

read_method_spec!(
    /// Get market configuration.
    "market.getConfiguration": GetConfiguration(GetConfigurationParams) -> GetConfigurationResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsParams {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsResult {
    pub positions: HashMap<AccountId, BorrowPosition>,
}

read_method_spec!(
    /// List borrow positions.
    "market.listBorrowPositions": ListBorrowPositions(ListBorrowPositionsParams) -> ListBorrowPositionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCurrentSnapshotParams {
    pub market_id: AccountId,
}

pub type GetCurrentSnapshotResult = Snapshot;

read_method_spec!(
    /// Get the current market snapshot.
    "market.getCurrentSnapshot": GetCurrentSnapshot(GetCurrentSnapshotParams) -> GetCurrentSnapshotResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetFinalizedSnapshotsLenParams {
    pub market_id: AccountId,
}

pub type GetFinalizedSnapshotsLenResult = u32;

read_method_spec!(
    /// Get finalized snapshot count.
    "market.getFinalizedSnapshotsLen": GetFinalizedSnapshotsLen(GetFinalizedSnapshotsLenParams) -> GetFinalizedSnapshotsLenResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFinalizedSnapshotsParams {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFinalizedSnapshotsResult {
    pub snapshots: Vec<Snapshot>,
}

read_method_spec!(
    /// List finalized snapshots.
    "market.listFinalizedSnapshots": ListFinalizedSnapshots(ListFinalizedSnapshotsParams) -> ListFinalizedSnapshotsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowAssetMetricsParams {
    pub market_id: AccountId,
}

pub type GetBorrowAssetMetricsResult = BorrowAssetMetrics;

read_method_spec!(
    /// Get borrow asset metrics.
    "market.getBorrowAssetMetrics": GetBorrowAssetMetrics(GetBorrowAssetMetricsParams) -> GetBorrowAssetMetricsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionResult {
    pub position: Option<BorrowPosition>,
}

read_method_spec!(
    /// Get a borrow position.
    "market.getBorrowPosition": GetBorrowPosition(GetBorrowPositionParams) -> GetBorrowPositionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionPendingInterestParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionPendingInterestResult {
    pub amount: Option<BorrowAssetAmount>,
}

read_method_spec!(
    /// Get pending borrow interest.
    "market.getBorrowPositionPendingInterest": GetBorrowPositionPendingInterest(GetBorrowPositionPendingInterestParams) -> GetBorrowPositionPendingInterestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowStatusParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub oracle_response: OracleResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowStatusResult {
    pub status: Option<BorrowStatus>,
}

read_method_spec!(
    /// Get borrow status for an account.
    "market.getBorrowStatus": GetBorrowStatus(GetBorrowStatusParams) -> GetBorrowStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListSupplyPositionsParams {
    pub market_id: AccountId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListSupplyPositionsResult {
    pub positions: HashMap<AccountId, SupplyPosition>,
}

read_method_spec!(
    /// List supply positions.
    "market.listSupplyPositions": ListSupplyPositions(ListSupplyPositionsParams) -> ListSupplyPositionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionResult {
    pub position: Option<SupplyPosition>,
}

read_method_spec!(
    /// Get a supply position.
    "market.getSupplyPosition": GetSupplyPosition(GetSupplyPositionParams) -> GetSupplyPositionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionPendingYieldParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionPendingYieldResult {
    pub amount: Option<BorrowAssetAmount>,
}

read_method_spec!(
    /// Get pending supply yield.
    "market.getSupplyPositionPendingYield": GetSupplyPositionPendingYield(GetSupplyPositionPendingYieldParams) -> GetSupplyPositionPendingYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalRequestStatusParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalRequestStatusResult {
    pub status: Option<WithdrawalRequestStatus>,
}

read_method_spec!(
    /// Get supply withdrawal request status.
    "market.getSupplyWithdrawalRequestStatus": GetSupplyWithdrawalRequestStatus(GetSupplyWithdrawalRequestStatusParams) -> GetSupplyWithdrawalRequestStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalQueueStatusParams {
    pub market_id: AccountId,
}

pub type GetSupplyWithdrawalQueueStatusResult = WithdrawalQueueStatus;

read_method_spec!(
    /// Get supply withdrawal queue status.
    "market.getSupplyWithdrawalQueueStatus": GetSupplyWithdrawalQueueStatus(GetSupplyWithdrawalQueueStatusParams) -> GetSupplyWithdrawalQueueStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetLastYieldRateParams {
    pub market_id: AccountId,
}

pub type GetLastYieldRateResult = Decimal;

read_method_spec!(
    /// Get the last yield rate.
    "market.getLastYieldRate": GetLastYieldRate(GetLastYieldRateParams) -> GetLastYieldRateResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStaticYieldParams {
    pub market_id: AccountId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStaticYieldResult {
    /// Total accumulated static yield denominated in the borrow asset, computed
    /// regardless of the market version's on-chain representation.
    pub borrow_asset_total: BorrowAssetAmount,
    /// The yield accumulator, present only for markets that expose it
    /// (>= 1.1.0). `None` for legacy markets that report a split record.
    pub accumulator: Option<Accumulator<BorrowAsset>>,
}

read_method_spec!(
    /// Get accumulated static yield.
    "market.getStaticYield": GetStaticYield(GetStaticYieldParams) -> GetStaticYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BorrowBody {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}
write_method_spec!(
    /// Borrow from a market.
    "market.borrow": Borrow(BorrowBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub registry_id: AccountId,
    pub name: String,
    pub version_key: String,
    pub configuration: MarketConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}
write_method_spec!(
    /// Create a market from the registry.
    "market.create": Create(CreateBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupplyBody {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}
write_method_spec!(
    /// Supply assets to a market.
    "market.supply": Supply(SupplyBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawCollateralBody {
    pub market_id: AccountId,
    pub amount: CollateralAssetAmount,
}
write_method_spec!(
    /// Withdraw collateral from a market.
    "market.withdrawCollateral": WithdrawCollateral(WithdrawCollateralBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApplyInterestBody {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub snapshot_limit: Option<u32>,
}
write_method_spec!(
    /// Apply interest to a market account.
    "market.applyInterest": ApplyInterest(ApplyInterestBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepayBody {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
    pub account_id: Option<AccountId>,
}
write_method_spec!(
    /// Repay borrowed assets.
    "market.repay": Repay(RepayBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateSupplyWithdrawalRequestBody {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
}
write_method_spec!(
    /// Create a supply withdrawal request.
    "market.createSupplyWithdrawalRequest": CreateSupplyWithdrawalRequest(CreateSupplyWithdrawalRequestBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelSupplyWithdrawalRequestBody {
    pub market_id: AccountId,
}
write_method_spec!(
    /// Cancel a supply withdrawal request.
    "market.cancelSupplyWithdrawalRequest": CancelSupplyWithdrawalRequest(CancelSupplyWithdrawalRequestBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteNextSupplyWithdrawalRequestBody {
    pub market_id: AccountId,
    pub batch_limit: Option<u32>,
}
write_method_spec!(
    /// Execute the next supply withdrawal request.
    "market.executeNextSupplyWithdrawalRequest": ExecuteNextSupplyWithdrawalRequest(ExecuteNextSupplyWithdrawalRequestBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawSupplyBody {
    pub market_id: AccountId,
    pub amount: BorrowAssetAmount,
    pub batch_limit: Option<u32>,
}
write_method_spec!(
    /// Withdraw supplied assets.
    "market.withdrawSupply": WithdrawSupply(WithdrawSupplyBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiquidateBody {
    pub market_id: AccountId,
    pub account_id: AccountId,
    pub liquidation_amount: BorrowAssetAmount,
    pub collateral_amount: Option<CollateralAssetAmount>,
}
write_method_spec!(
    /// Liquidate an unhealthy account.
    "market.liquidate": Liquidate(LiquidateBody)
);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HarvestYieldBody {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub mode: Option<HarvestYieldMode>,
}
write_method_spec!(
    /// Harvest market yield.
    "market.harvestYield": HarvestYield(HarvestYieldBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccumulateStaticYieldBody {
    pub market_id: AccountId,
    pub account_id: Option<AccountId>,
    pub snapshot_limit: Option<u32>,
}
write_method_spec!(
    /// Accumulate static yield.
    "market.accumulateStaticYield": AccumulateStaticYield(AccumulateStaticYieldBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawStaticYieldBody {
    pub market_id: AccountId,
    pub amount: Option<BorrowAssetAmount>,
}
write_method_spec!(
    /// Withdraw static yield.
    "market.withdrawStaticYield": WithdrawStaticYield(WithdrawStaticYieldBody)
);
