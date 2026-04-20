use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    primitive::PublicKey,
    rpc::common::Pagination,
    rpc::common::WriteOperationResult,
    MarketId, NearToken, RegistryId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConfigurationParams {
    pub market_id: MarketId,
}

pub type GetConfigurationResult = MarketConfiguration;

public_read_method_spec!(
    GetConfiguration,
    "market.getConfiguration",
    GetConfigurationParams,
    GetConfigurationResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsParams {
    pub market_id: MarketId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBorrowPositionsResult {
    pub positions: HashMap<near_account_id::AccountId, BorrowPosition>,
}

public_read_method_spec!(
    ListBorrowPositions,
    "market.listBorrowPositions",
    ListBorrowPositionsParams,
    ListBorrowPositionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCurrentSnapshotParams {
    pub market_id: MarketId,
}

pub type GetCurrentSnapshotResult = Snapshot;

public_read_method_spec!(
    GetCurrentSnapshot,
    "market.getCurrentSnapshot",
    GetCurrentSnapshotParams,
    GetCurrentSnapshotResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetFinalizedSnapshotsLenParams {
    pub market_id: MarketId,
}

pub type GetFinalizedSnapshotsLenResult = u32;

public_read_method_spec!(
    GetFinalizedSnapshotsLen,
    "market.getFinalizedSnapshotsLen",
    GetFinalizedSnapshotsLenParams,
    GetFinalizedSnapshotsLenResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFinalizedSnapshotsParams {
    pub market_id: MarketId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFinalizedSnapshotsResult {
    pub snapshots: Vec<Snapshot>,
}

public_read_method_spec!(
    ListFinalizedSnapshots,
    "market.listFinalizedSnapshots",
    ListFinalizedSnapshotsParams,
    ListFinalizedSnapshotsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowAssetMetricsParams {
    pub market_id: MarketId,
}

pub type GetBorrowAssetMetricsResult = BorrowAssetMetrics;

public_read_method_spec!(
    GetBorrowAssetMetrics,
    "market.getBorrowAssetMetrics",
    GetBorrowAssetMetricsParams,
    GetBorrowAssetMetricsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionResult {
    pub position: Option<BorrowPosition>,
}

public_read_method_spec!(
    GetBorrowPosition,
    "market.getBorrowPosition",
    GetBorrowPositionParams,
    GetBorrowPositionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionPendingInterestParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowPositionPendingInterestResult {
    pub amount: Option<BorrowAssetAmount>,
}

public_read_method_spec!(
    GetBorrowPositionPendingInterest,
    "market.getBorrowPositionPendingInterest",
    GetBorrowPositionPendingInterestParams,
    GetBorrowPositionPendingInterestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowStatusParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
    pub oracle_response: OracleResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetBorrowStatusResult {
    pub status: Option<BorrowStatus>,
}

public_read_method_spec!(
    GetBorrowStatus,
    "market.getBorrowStatus",
    GetBorrowStatusParams,
    GetBorrowStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListSupplyPositionsParams {
    pub market_id: MarketId,
    #[serde(flatten)]
    pub args: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListSupplyPositionsResult {
    pub positions: HashMap<near_account_id::AccountId, SupplyPosition>,
}

public_read_method_spec!(
    ListSupplyPositions,
    "market.listSupplyPositions",
    ListSupplyPositionsParams,
    ListSupplyPositionsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionResult {
    pub position: Option<SupplyPosition>,
}

public_read_method_spec!(
    GetSupplyPosition,
    "market.getSupplyPosition",
    GetSupplyPositionParams,
    GetSupplyPositionResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionPendingYieldParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
    pub snapshot_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyPositionPendingYieldResult {
    pub amount: Option<BorrowAssetAmount>,
}

public_read_method_spec!(
    GetSupplyPositionPendingYield,
    "market.getSupplyPositionPendingYield",
    GetSupplyPositionPendingYieldParams,
    GetSupplyPositionPendingYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalRequestStatusParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalRequestStatusResult {
    pub status: Option<WithdrawalRequestStatus>,
}

public_read_method_spec!(
    GetSupplyWithdrawalRequestStatus,
    "market.getSupplyWithdrawalRequestStatus",
    GetSupplyWithdrawalRequestStatusParams,
    GetSupplyWithdrawalRequestStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetSupplyWithdrawalQueueStatusParams {
    pub market_id: MarketId,
}

pub type GetSupplyWithdrawalQueueStatusResult = WithdrawalQueueStatus;

public_read_method_spec!(
    GetSupplyWithdrawalQueueStatus,
    "market.getSupplyWithdrawalQueueStatus",
    GetSupplyWithdrawalQueueStatusParams,
    GetSupplyWithdrawalQueueStatusResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetLastYieldRateParams {
    pub market_id: MarketId,
}

pub type GetLastYieldRateResult = Decimal;

public_read_method_spec!(
    GetLastYieldRate,
    "market.getLastYieldRate",
    GetLastYieldRateParams,
    GetLastYieldRateResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStaticYieldParams {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetStaticYieldResult {
    pub accumulator: Option<Accumulator<BorrowAsset>>,
}

public_read_method_spec!(
    GetStaticYield,
    "market.getStaticYield",
    GetStaticYieldParams,
    GetStaticYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BorrowBody {
    pub market_id: MarketId,
    pub amount: BorrowAssetAmount,
}
pub type BorrowResult = WriteOperationResult;
write_method_spec!(Borrow, "market.borrow", BorrowBody, BorrowResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub registry_id: RegistryId,
    pub name: String,
    pub version_key: String,
    pub configuration: MarketConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_keys: Option<Vec<PublicKey>>,
    pub deposit: NearToken,
}
pub type CreateResult = WriteOperationResult;
write_method_spec!(Create, "market.create", CreateBody, CreateResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupplyBody {
    pub market_id: MarketId,
    pub amount: BorrowAssetAmount,
}
pub type SupplyResult = WriteOperationResult;
write_method_spec!(Supply, "market.supply", SupplyBody, SupplyResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawCollateralBody {
    pub market_id: MarketId,
    pub amount: CollateralAssetAmount,
}
pub type WithdrawCollateralResult = WriteOperationResult;
write_method_spec!(
    WithdrawCollateral,
    "market.withdrawCollateral",
    WithdrawCollateralBody,
    WithdrawCollateralResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApplyInterestBody {
    pub market_id: MarketId,
    pub account_id: Option<near_account_id::AccountId>,
    pub snapshot_limit: Option<u32>,
}
pub type ApplyInterestResult = WriteOperationResult;
write_method_spec!(
    ApplyInterest,
    "market.applyInterest",
    ApplyInterestBody,
    ApplyInterestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepayBody {
    pub market_id: MarketId,
    pub amount: BorrowAssetAmount,
    pub account_id: Option<near_account_id::AccountId>,
}
pub type RepayResult = WriteOperationResult;
write_method_spec!(Repay, "market.repay", RepayBody, RepayResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateSupplyWithdrawalRequestBody {
    pub market_id: MarketId,
    pub amount: BorrowAssetAmount,
}
pub type CreateSupplyWithdrawalRequestResult = WriteOperationResult;
write_method_spec!(
    CreateSupplyWithdrawalRequest,
    "market.createSupplyWithdrawalRequest",
    CreateSupplyWithdrawalRequestBody,
    CreateSupplyWithdrawalRequestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelSupplyWithdrawalRequestBody {
    pub market_id: MarketId,
}
pub type CancelSupplyWithdrawalRequestResult = WriteOperationResult;
write_method_spec!(
    CancelSupplyWithdrawalRequest,
    "market.cancelSupplyWithdrawalRequest",
    CancelSupplyWithdrawalRequestBody,
    CancelSupplyWithdrawalRequestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteNextSupplyWithdrawalRequestBody {
    pub market_id: MarketId,
    pub batch_limit: Option<u32>,
}
pub type ExecuteNextSupplyWithdrawalRequestResult = WriteOperationResult;
write_method_spec!(
    ExecuteNextSupplyWithdrawalRequest,
    "market.executeNextSupplyWithdrawalRequest",
    ExecuteNextSupplyWithdrawalRequestBody,
    ExecuteNextSupplyWithdrawalRequestResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawSupplyBody {
    pub market_id: MarketId,
    pub amount: BorrowAssetAmount,
    pub batch_limit: Option<u32>,
}
pub type WithdrawSupplyResult = WriteOperationResult;
write_method_spec!(
    WithdrawSupply,
    "market.withdrawSupply",
    WithdrawSupplyBody,
    WithdrawSupplyResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiquidateBody {
    pub market_id: MarketId,
    pub account_id: near_account_id::AccountId,
    pub liquidation_amount: BorrowAssetAmount,
    pub collateral_amount: Option<CollateralAssetAmount>,
}
pub type LiquidateResult = WriteOperationResult;
write_method_spec!(
    Liquidate,
    "market.liquidate",
    LiquidateBody,
    LiquidateResult
);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HarvestYieldBody {
    pub market_id: MarketId,
    pub account_id: Option<near_account_id::AccountId>,
    pub mode: Option<HarvestYieldMode>,
}
pub type HarvestYieldResult = WriteOperationResult;
write_method_spec!(
    HarvestYield,
    "market.harvestYield",
    HarvestYieldBody,
    HarvestYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccumulateStaticYieldBody {
    pub market_id: MarketId,
    pub account_id: Option<near_account_id::AccountId>,
    pub snapshot_limit: Option<u32>,
}
pub type AccumulateStaticYieldResult = WriteOperationResult;
write_method_spec!(
    AccumulateStaticYield,
    "market.accumulateStaticYield",
    AccumulateStaticYieldBody,
    AccumulateStaticYieldResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WithdrawStaticYieldBody {
    pub market_id: MarketId,
    pub amount: Option<BorrowAssetAmount>,
}
pub type WithdrawStaticYieldResult = WriteOperationResult;
write_method_spec!(
    WithdrawStaticYield,
    "market.withdrawStaticYield",
    WithdrawStaticYieldBody,
    WithdrawStaticYieldResult
);
