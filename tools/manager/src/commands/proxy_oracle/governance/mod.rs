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
use clap::{Args, Subcommand};
use near_account_id::AccountId;
use serde::de::DeserializeOwned;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;
use templar_gateway_types::Base64Bytes;
use templar_proxy_oracle_near_governance_common::TtlConfig;
// Re-exported so the leaf command modules parse them directly as `clap::ValueEnum`
// (derived upstream behind the crate's `clap` feature), avoiding a local mirror.
pub use templar_proxy_oracle_near_governance_common::{OperationKind, Role};

use crate::commands::signer::SignerArgs;
use crate::resolve::GovernanceTarget;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum ProxyOracleGovernanceNs {
    /// Deploy a governance contract from a registered version.
    Create(GovernanceCreate),
    /// Create a governance proposal.
    CreateProposal(CreateProposal),
    /// Cancel a pending proposal.
    CancelProposal(CancelProposal),
    /// Execute a matured proposal.
    ExecuteProposal(ExecuteProposalArgs),
    /// Read a single proposal.
    GetProposal(ProposalRef),
    /// List the governance contract's proposals.
    ListProposals(ListProposals),
    /// Read the id the next proposal will use.
    NextProposalId(GovernanceTarget),
    /// Read the total number of proposals.
    ProposalCount(GovernanceTarget),
    /// Read the configured TTL for an operation kind.
    GetOperationTtl(GetOperationTtl),
    /// Read the proxy-oracle account this contract governs.
    GetProxyOracleId(GovernanceTarget),
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
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Proposal id.
    #[arg(long, value_name = "ID")]
    id: u32,
}

impl ProposalRef {
    pub fn get(self, governance_id: AccountId) -> spec::GetProposal {
        spec::GetProposal {
            governance_id,
            id: self.id,
        }
    }
}

/// `cancel-proposal`: a proposal reference plus the signer credentials the write
/// requires. Distinct from the read-only `get-proposal`, which reuses the bare
/// [`ProposalRef`] and so carries no credentials.
#[derive(Args, Debug)]
pub struct CancelProposal {
    #[command(flatten)]
    pub(crate) proposal: ProposalRef,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl CancelProposal {
    pub fn cancel(self, governance_id: AccountId) -> spec::CancelProposal {
        spec::CancelProposal {
            governance_id,
            id: self.proposal.id,
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
        self_upgrade: ttl,
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
