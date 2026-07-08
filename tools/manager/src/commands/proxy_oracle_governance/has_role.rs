use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::RoleArg;

#[derive(Args, Debug)]
pub struct HasRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
}

impl HasRole {
    pub fn parse(self) -> spec::HasRole {
        spec::HasRole {
            governance_id: self.governance_id,
            account_id: self.account_id,
            role: self.role.into(),
        }
    }
}
