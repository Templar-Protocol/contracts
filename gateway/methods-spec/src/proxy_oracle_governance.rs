use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::Nanoseconds;
use templar_gateway_macros::{read_method_spec, write_method_spec};
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Proposal};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NextProposalId {
    pub governance_id: near_account_id::AccountId,
}
pub type NextProposalIdResult = u32;
read_method_spec!(
    /// Get the next governance proposal ID.
    "proxyOracleGovernance.nextProposalId": NextProposalId -> NextProposalIdResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposalCount {
    pub governance_id: near_account_id::AccountId,
}
pub type ProposalCountResult = u32;
read_method_spec!(
    /// Get the count of active governance proposals.
    "proxyOracleGovernance.proposalCount": ProposalCount -> ProposalCountResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOperationTtl {
    pub governance_id: near_account_id::AccountId,
    pub kind: OperationKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOperationTtlResult {
    pub ttl_ns: Nanoseconds,
}
read_method_spec!(
    /// Get the configured proposal TTL for an operation kind.
    "proxyOracleGovernance.getOperationTtl": GetOperationTtl -> GetOperationTtlResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProposals {
    pub governance_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProposalsResult {
    pub ids: Vec<u32>,
}
read_method_spec!(
    /// List active governance proposal IDs.
    "proxyOracleGovernance.listProposals": ListProposals -> ListProposalsResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposalResult {
    pub proposal: Option<Proposal<Operation>>,
}
read_method_spec!(
    /// Get a governance proposal.
    "proxyOracleGovernance.getProposal": GetProposal -> GetProposalResult
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
    pub operation: Operation,
    pub requested_ttl: Nanoseconds,
}
write_method_spec!(
    /// Create a governance proposal.
    "proxyOracleGovernance.createProposal": CreateProposal
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}
write_method_spec!(
    /// Cancel a governance proposal.
    "proxyOracleGovernance.cancelProposal": CancelProposal
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}
write_method_spec!(
    /// Execute a governance proposal.
    "proxyOracleGovernance.executeProposal": ExecuteProposal
);
