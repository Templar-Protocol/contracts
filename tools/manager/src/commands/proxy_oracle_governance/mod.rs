mod create;
mod create_proposal;
mod execute_proposal;
mod get_operation_ttl;
mod get_roles;
mod has_role;
mod list_proposals;
mod list_role;

pub use create::GovernanceCreate;
pub use create_proposal::CreateProposal;
pub use execute_proposal::ExecuteProposalArgs;
pub use get_operation_ttl::GetOperationTtl;
pub use get_roles::GetRoles;
pub use has_role::HasRole;
pub use list_proposals::ListProposals;
pub use list_role::ListRole;

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};
use near_account_id::AccountId;
use serde::de::DeserializeOwned;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;
use templar_gateway_types::Base64Bytes;
use templar_proxy_oracle_near_governance_common::{OperationKind, Role, TtlConfig};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleGovernanceNs {
    /// Deploy a governance contract from a registered version.
    Create(GovernanceCreate),
    /// Create a governance proposal.
    CreateProposal(CreateProposal),
    /// Cancel a pending proposal.
    CancelProposal(ProposalRef),
    /// Execute a matured proposal.
    ExecuteProposal(ExecuteProposalArgs),
    /// Read a single proposal.
    GetProposal(ProposalRef),
    /// List the governance contract's proposals.
    ListProposals(ListProposals),
    /// Read the id the next proposal will use.
    NextProposalId(GovernanceIdArgs),
    /// Read the total number of proposals.
    ProposalCount(GovernanceIdArgs),
    /// Read the configured TTL for an operation kind.
    GetOperationTtl(GetOperationTtl),
    /// Read the proxy-oracle account this contract governs.
    GetProxyOracleId(GovernanceIdArgs),
    /// Check whether an account holds a role.
    HasRole(HasRole),
    /// List the accounts holding a role.
    ListRole(ListRole),
    /// List the roles held by an account.
    GetRoles(GetRoles),
}

/// A governance proposal keyed by id — shared by `cancel-proposal` and
/// `get-proposal`.
#[derive(Args, Debug)]
pub struct ProposalRef {
    /// Governance contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    /// Proposal id.
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl ProposalRef {
    pub fn cancel(self) -> spec::CancelProposal {
        spec::CancelProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
    pub fn get(self) -> spec::GetProposal {
        spec::GetProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}

/// Argument keyed only by the governance account — shared by the reads that take
/// no other input (`next-proposal-id`, `proposal-count`, `get-proxy-oracle-id`).
#[derive(Args, Debug)]
pub struct GovernanceIdArgs {
    /// Governance contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
}

impl GovernanceIdArgs {
    pub fn next_proposal_id(self) -> spec::NextProposalId {
        spec::NextProposalId {
            governance_id: self.governance_id,
        }
    }
    pub fn proposal_count(self) -> spec::ProposalCount {
        spec::ProposalCount {
            governance_id: self.governance_id,
        }
    }
    pub fn get_proxy_oracle_id(self) -> spec::GetProxyOracleId {
        spec::GetProxyOracleId {
            governance_id: self.governance_id,
        }
    }
}

/// Local clap mirror of `Role` (keeps governance-common clap-free).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RoleArg {
    ManualTripper,
    CircuitBreakerOperator,
    ProxyConfigurationManager,
    Admin,
}

impl From<RoleArg> for Role {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::ManualTripper => Self::ManualTripper,
            RoleArg::CircuitBreakerOperator => Self::CircuitBreakerOperator,
            RoleArg::ProxyConfigurationManager => Self::ProxyConfigurationManager,
            RoleArg::Admin => Self::Admin,
        }
    }
}

/// Local clap mirror of `OperationKind`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OperationKindArg {
    SetProxy,
    ConfigureCircuitBreakers,
    AddCircuitBreaker,
    RemoveCircuitBreaker,
    SetManualTrip,
    Rearm,
    SetEnforced,
    SetActionTtl,
    SetRole,
    AdminUpgrade,
    AdminFunctionCall,
}

impl From<OperationKindArg> for OperationKind {
    fn from(kind: OperationKindArg) -> Self {
        match kind {
            OperationKindArg::SetProxy => Self::SetProxy,
            OperationKindArg::ConfigureCircuitBreakers => Self::ConfigureCircuitBreakers,
            OperationKindArg::AddCircuitBreaker => Self::AddCircuitBreaker,
            OperationKindArg::RemoveCircuitBreaker => Self::RemoveCircuitBreaker,
            OperationKindArg::SetManualTrip => Self::SetManualTrip,
            OperationKindArg::Rearm => Self::Rearm,
            OperationKindArg::SetEnforced => Self::SetEnforced,
            OperationKindArg::SetActionTtl => Self::SetActionTtl,
            OperationKindArg::SetRole => Self::SetRole,
            OperationKindArg::AdminUpgrade => Self::AdminUpgrade,
            OperationKindArg::AdminFunctionCall => Self::AdminFunctionCall,
        }
    }
}

fn uniform_ttls(ttl: Nanoseconds) -> TtlConfig {
    TtlConfig {
        set_proxy: ttl,
        configure_circuit_breakers: ttl,
        add_circuit_breaker: ttl,
        remove_circuit_breaker: ttl,
        set_manual_trip: ttl,
        rearm: ttl,
        set_enforced: ttl,
        set_action_ttl: ttl,
        set_role: ttl,
        admin_upgrade: ttl,
        admin_function_call: ttl,
    }
}

fn load_json_file<T: DeserializeOwned>(path: &std::path::Path) -> anyhow::Result<T> {
    let contents =
        std::fs::read(path).with_context(|| format!("read JSON from {}", path.display()))?;
    serde_json::from_slice(&contents).with_context(|| format!("parse JSON from {}", path.display()))
}

fn decode_base64(value: String) -> anyhow::Result<Vec<u8>> {
    // Reuse Base64Bytes' base64 deserializer rather than adding a base64 dep.
    let bytes: Base64Bytes = serde_json::from_value(serde_json::Value::String(value))
        .context("decode base64 metadata")?;
    Ok(bytes.0)
}
