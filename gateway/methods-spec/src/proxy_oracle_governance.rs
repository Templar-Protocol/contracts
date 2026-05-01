use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::{
    governance::Proposal, oracle::proxy::governance::Operation, time::Nanoseconds,
};
use templar_gateway_macros::{read_method_spec, write_method_spec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetNextIdParams {
    pub oracle_id: near_account_id::AccountId,
}
pub type GetNextIdResult = u32;
read_method_spec!(
    /// Get the next governance proposal ID.
    "proxyOracleGovernance.getNextId": GetNextId(GetNextIdParams) -> GetNextIdResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTtlParams {
    pub oracle_id: near_account_id::AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetTtlResult {
    pub ttl_ns: Nanoseconds,
}
read_method_spec!(
    /// Get governance proposal TTL.
    "proxyOracleGovernance.getTtl": GetTtl(GetTtlParams) -> GetTtlResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetCountParams {
    pub oracle_id: near_account_id::AccountId,
}
pub type GetCountResult = u32;
read_method_spec!(
    /// Get governance proposal count.
    "proxyOracleGovernance.getCount": GetCount(GetCountParams) -> GetCountResult
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
read_method_spec!(
    /// List governance proposal IDs.
    "proxyOracleGovernance.list": List(ListParams) -> ListResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetParams {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetResult {
    pub proposal: Option<Proposal<Operation>>,
}
read_method_spec!(
    /// Get a governance proposal.
    "proxyOracleGovernance.get": Get(GetParams) -> GetResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
    pub operation: Operation,
}
write_method_spec!(
    /// Create a governance proposal.
    "proxyOracleGovernance.create": Create(CreateBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}
write_method_spec!(
    /// Cancel a governance proposal.
    "proxyOracleGovernance.cancel": Cancel(CancelBody)
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteBody {
    pub oracle_id: near_account_id::AccountId,
    pub id: u32,
}
write_method_spec!(
    /// Execute a governance proposal.
    "proxyOracleGovernance.execute": Execute(ExecuteBody)
);
