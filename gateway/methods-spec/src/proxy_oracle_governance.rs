use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::Nanoseconds;
use templar_gateway_macros::MethodSpec;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Proposal};

/// Get the next governance proposal ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.nextProposalId", output = NextProposalIdResult)]
pub struct NextProposalId {
    pub governance_id: near_account_id::AccountId,
}
pub type NextProposalIdResult = u32;

/// Get the count of active governance proposals.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.proposalCount", output = ProposalCountResult)]
pub struct ProposalCount {
    pub governance_id: near_account_id::AccountId,
}
pub type ProposalCountResult = u32;

/// Get the configured proposal TTL for an operation kind.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.getOperationTtl", output = GetOperationTtlResult)]
pub struct GetOperationTtl {
    pub governance_id: near_account_id::AccountId,
    pub kind: OperationKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetOperationTtlResult {
    pub ttl_ns: Nanoseconds,
}

/// List active governance proposal IDs.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.listProposals", output = ListProposalsResult)]
pub struct ListProposals {
    pub governance_id: near_account_id::AccountId,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProposalsResult {
    pub ids: Vec<u32>,
}

/// Get a governance proposal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.getProposal", output = GetProposalResult)]
pub struct GetProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProposalResult {
    pub proposal: Option<Proposal<Operation>>,
}

/// Create a governance proposal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleGovernance.createProposal")]
pub struct CreateProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
    pub operation: Operation,
    pub requested_ttl: Nanoseconds,
}

/// Cancel a governance proposal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleGovernance.cancelProposal")]
pub struct CancelProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}

/// Execute a governance proposal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleGovernance.executeProposal")]
pub struct ExecuteProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
}
