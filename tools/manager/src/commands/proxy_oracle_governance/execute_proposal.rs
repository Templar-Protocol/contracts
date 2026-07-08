use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

#[derive(Args, Debug)]
pub struct ExecuteProposalArgs {
    /// Governance contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    /// Proposal id to execute.
    #[arg(long, value_name = "ID")]
    id: u32,
    /// Wait for the proposal's TTL to elapse before executing, instead of
    /// failing if it has not yet matured.
    #[arg(long)]
    when_ready: bool,
}

impl ExecuteProposalArgs {
    pub fn governance_id(&self) -> &AccountId {
        &self.governance_id
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn when_ready(&self) -> bool {
        self.when_ready
    }
    pub fn into_spec(self) -> spec::ExecuteProposal {
        spec::ExecuteProposal {
            governance_id: self.governance_id,
            id: self.id,
        }
    }
}
