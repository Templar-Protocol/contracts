use near_account_id::AccountId;
use near_openapi_types::TxExecutionStatus as NearTxExecutionStatus;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{IdempotencyKey, ManagedAccountId, OperationOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TxExecutionStatus {
    None,
    Included,
    #[default]
    ExecutedOptimistic,
    IncludedFinal,
    Executed,
    Final,
}

impl From<TxExecutionStatus> for NearTxExecutionStatus {
    fn from(value: TxExecutionStatus) -> Self {
        match value {
            TxExecutionStatus::None => Self::None,
            TxExecutionStatus::Included => Self::Included,
            TxExecutionStatus::ExecutedOptimistic => Self::ExecutedOptimistic,
            TxExecutionStatus::IncludedFinal => Self::IncludedFinal,
            TxExecutionStatus::Executed => Self::Executed,
            TxExecutionStatus::Final => Self::Final,
        }
    }
}

impl From<NearTxExecutionStatus> for TxExecutionStatus {
    fn from(value: NearTxExecutionStatus) -> Self {
        match value {
            NearTxExecutionStatus::None => Self::None,
            NearTxExecutionStatus::Included => Self::Included,
            NearTxExecutionStatus::ExecutedOptimistic => Self::ExecutedOptimistic,
            NearTxExecutionStatus::IncludedFinal => Self::IncludedFinal,
            NearTxExecutionStatus::Executed => Self::Executed,
            NearTxExecutionStatus::Final => Self::Final,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct Pagination {
    pub offset: Option<u32>,
    #[serde(rename = "count", alias = "limit")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum ContractArgs {
    Json(serde_json::Value),
    Raw(crate::Base64Bytes),
}

impl ContractArgs {
    pub fn try_into_bytes(self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::Json(value) => serde_json::to_vec(&value),
            Self::Raw(bytes) => Ok(bytes.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ReadRequest<T> {
    #[serde(flatten)]
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WriteRequest<T> {
    pub signer_account_id: ManagedAccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
    #[serde(default)]
    pub wait_until: TxExecutionStatus,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WriteOperationResult {
    pub outcome: OperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageBalanceBounds {
    pub min: crate::NearToken,
    pub max: Option<crate::NearToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageBalance {
    pub total: crate::NearToken,
    pub available: crate::NearToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountIdList {
    pub account_ids: Vec<AccountId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StringList {
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonValueResult {
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonValueListResult {
    pub values: Vec<serde_json::Value>,
}
