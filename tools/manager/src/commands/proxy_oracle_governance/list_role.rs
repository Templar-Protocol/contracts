use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::Role;
use crate::commands::pagination::PaginationArgs;

#[derive(Args, Debug)]
pub struct ListRole {
    /// Governance contract to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    /// Role whose members to list.
    #[arg(long, value_enum)]
    role: Role,
    #[command(flatten)]
    pagination: PaginationArgs,
}

impl ListRole {
    pub fn into_spec(self) -> spec::ListRole {
        spec::ListRole {
            governance_id: self.governance_id,
            role: self.role,
            offset: self.pagination.offset,
            count: self.pagination.count,
        }
    }
}
