use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::{
    governance::Proposal, oracle::proxy::governance::Operation, time::Nanoseconds,
};

use crate::{
    macros::{public_read_method_spec, write_method_spec},
    rpc::common::WriteOperationResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetNextIdParams {
    pub oracle_id: near_account_id::AccountId,
}
pub type GetNextIdResult = u32;
public_read_method_spec!(
    GetNextId,
    "proxyOracleGovernance.getNextId",
    GetNextIdParams,
    GetNextIdResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTtlParams {
    pub oracle_id: near_account_id::AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTtlResult {
    pub ttl_ns: Nanoseconds,
}
public_read_method_spec!(
    GetTtl,
    "proxyOracleGovernance.getTtl",
    GetTtlParams,
    GetTtlResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCountParams {
    pub oracle_id: near_account_id::AccountId,
}
pub type GetCountResult = u32;
public_read_method_spec!(
    GetCount,
    "proxyOracleGovernance.getCount",
    GetCountParams,
    GetCountResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListParams {
    pub oracle_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListResult {
    pub ids: Vec<u32>,
}
public_read_method_spec!(List, "proxyOracleGovernance.list", ListParams, ListResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub proposal: Option<Proposal<Operation>>,
}
public_read_method_spec!(Get, "proxyOracleGovernance.get", GetParams, GetResult);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
    pub operation: Operation,
}
pub type CreateResult = WriteOperationResult;
write_method_spec!(
    Create,
    "proxyOracleGovernance.create",
    CreateBody,
    CreateResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}
pub type CancelResult = WriteOperationResult;
write_method_spec!(
    Cancel,
    "proxyOracleGovernance.cancel",
    CancelBody,
    CancelResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}
pub type ExecuteResult = WriteOperationResult;
write_method_spec!(
    Execute,
    "proxyOracleGovernance.execute",
    ExecuteBody,
    ExecuteResult
);
