use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListProposals {
    /// Governance contract to list proposals from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListProposals {
    pub fn into_spec(self) -> spec::ListProposals {
        spec::ListProposals {
            governance_id: self.governance_id,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
