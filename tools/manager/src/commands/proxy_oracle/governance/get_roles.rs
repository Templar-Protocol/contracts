use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use crate::resolve::GovernanceTarget;

#[derive(Args, Debug)]
pub struct GetRoles {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
    /// Account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetRoles {
    pub fn into_spec(self, governance_id: AccountId) -> spec::GetRoles {
        spec::GetRoles {
            governance_id,
            account_id: self.account_id,
        }
    }
}
