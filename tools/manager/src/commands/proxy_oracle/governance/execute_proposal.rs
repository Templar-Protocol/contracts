use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use crate::commands::proxy_oracle::PreflightArgs;
use crate::commands::signer::SignerArgs;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct ExecuteProposalArgs {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Proposal id to execute.
    #[arg(long, value_name = "ID")]
    id: u32,
    /// Wait for the proposal's TTL to elapse before executing, instead of
    /// failing if it has not yet matured.
    #[arg(long, conflicts_with = "print")]
    when_ready: bool,
    #[command(flatten)]
    pub(crate) preflight: PreflightArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl ExecuteProposalArgs {
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn when_ready(&self) -> bool {
        self.when_ready
    }
    pub fn into_spec(self, governance_id: AccountId) -> spec::ExecuteProposal {
        spec::ExecuteProposal {
            governance_id,
            id: self.id,
        }
    }
}
