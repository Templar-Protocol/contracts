use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use crate::commands::pagination::PaginationArgs;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct ListProposals {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListProposals {
    pub fn into_spec(self, governance_id: AccountId) -> spec::ListProposals {
        spec::ListProposals {
            governance_id,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
