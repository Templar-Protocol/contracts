use near_account_id::AccountId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::Nanoseconds;
use templar_gateway_macros::MethodSpec;
use templar_gateway_types::ProposalEncoding;
use templar_proxy_oracle_near_governance_common::{
    GovernancePolicy, GovernancePolicyWire, Operation, Proposal, Role,
};

/// Get the next governance proposal ID.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.nextProposalId", output = u32)]
pub struct NextProposalId {
    pub governance_id: near_account_id::AccountId,
}

/// Get the count of active governance proposals.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.proposalCount", output = u32)]
pub struct ProposalCount {
    pub governance_id: near_account_id::AccountId,
}

/// Get the governance policy table (reflexive timelocks, the conservative target default, and
/// per-method overrides).
///
/// Returned in its unconstrained wire form: the policy bounds are enforced when a policy is written,
/// and a read should surface whatever the contract actually holds rather than fail to decode state
/// that predates the current bounds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.getGovernancePolicy", output = GetGovernancePolicyResult)]
pub struct GetGovernancePolicy {
    pub governance_id: near_account_id::AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetGovernancePolicyResult {
    pub policy: GovernancePolicyWire,
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

/// Get the account id of the proxy oracle this governance contract governs.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.getProxyOracleId", output = GetProxyOracleIdResult)]
pub struct GetProxyOracleId {
    pub governance_id: AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetProxyOracleIdResult {
    pub proxy_oracle_id: AccountId,
}

/// Check whether an account holds a governance role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.hasRole", output = HasRoleResult)]
pub struct HasRole {
    pub governance_id: AccountId,
    pub account_id: AccountId,
    pub role: Role,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HasRoleResult {
    pub has_role: bool,
}

/// List the accounts holding a governance role.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.listRole", output = ListRoleResult)]
pub struct ListRole {
    pub governance_id: AccountId,
    pub role: Role,
    pub offset: Option<u32>,
    pub count: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListRoleResult {
    pub members: Vec<AccountId>,
}

/// Get every governance role an account holds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(read = "proxyOracleGovernance.getRoles", output = GetRolesResult)]
pub struct GetRoles {
    pub governance_id: AccountId,
    pub account_id: AccountId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetRolesResult {
    pub roles: Vec<Role>,
}

/// Create a proxy oracle governance contract from the registry.
///
/// A governance contract administers exactly one proxy oracle and must be that
/// oracle's owner, so deploy it before the oracle and name it as the oracle's
/// [`proxyOracle.create`](crate::proxy_oracle::Create) `owner_id`.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleGovernance.create")]
pub struct Create {
    #[serde(flatten)]
    pub target: crate::registry::DeployTarget,
    pub proxy_oracle_id: AccountId,
    pub admin_id: AccountId,
    pub policy: GovernancePolicy,
}

/// Create a governance proposal.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "proxyOracleGovernance.createProposal")]
pub struct CreateProposal {
    pub governance_id: near_account_id::AccountId,
    pub id: u32,
    pub operation: Operation,
    pub requested_ttl: Nanoseconds,
    #[serde(default, skip_serializing_if = "ProposalEncoding::is_json")]
    pub encoding: ProposalEncoding,
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

#[cfg(test)]
mod tests {
    use templar_common::Nanoseconds;
    use templar_proxy_oracle_near_governance_common::{Operation, ReflexiveOperation, Role};

    use super::{CreateProposal, ProposalEncoding};

    fn body(encoding: ProposalEncoding) -> CreateProposal {
        CreateProposal {
            governance_id: "gov.near".parse().unwrap(),
            id: 7,
            operation: Operation::Reflexive(ReflexiveOperation::SetRole {
                account_id: "op.near".parse().unwrap(),
                role: Role::Admin,
                set: true,
            }),
            requested_ttl: Nanoseconds::zero(),
            encoding,
        }
    }

    /// The persisted idempotency fingerprint hashes these params, so a default request must
    /// serialize as it did before `encoding` existed or retries stop matching their stored operation.
    #[test]
    fn the_default_encoding_stays_off_the_wire() {
        let json = serde_json::to_value(body(ProposalEncoding::Json)).unwrap();
        assert!(json.get("encoding").is_none(), "{json}");
        assert_eq!(
            serde_json::from_value::<CreateProposal>(json)
                .unwrap()
                .encoding,
            ProposalEncoding::Json
        );

        let opted_in = serde_json::to_value(body(ProposalEncoding::Borsh)).unwrap();
        assert_eq!(opted_in["encoding"], serde_json::json!("borsh"));
    }
}
