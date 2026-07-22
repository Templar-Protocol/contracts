use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::Role;
use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct HasRole {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    /// Role to check for.
    #[arg(long, value_enum)]
    role: Role,
}

impl HasRole {
    pub fn into_spec(self, governance_id: AccountId) -> spec::HasRole {
        spec::HasRole {
            governance_id,
            account_id: self.account_id,
            role: self.role,
        }
    }
}
