use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::Role;
use crate::commands::pagination::PaginationArgs;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct ListRole {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Role whose members to list.
    #[arg(long, value_enum)]
    role: Role,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListRole {
    pub fn into_spec(self, governance_id: AccountId) -> spec::ListRole {
        spec::ListRole {
            governance_id,
            role: self.role,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
