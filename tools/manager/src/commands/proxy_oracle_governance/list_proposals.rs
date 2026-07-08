use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

#[derive(Args, Debug)]
pub struct ListProposals {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListProposals {
    pub fn into_spec(self) -> spec::ListProposals {
        spec::ListProposals {
            governance_id: self.governance_id,
            offset: self.offset,
            count: self.count,
        }
    }
}
