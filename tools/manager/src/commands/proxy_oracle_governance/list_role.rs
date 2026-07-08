use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

use super::RoleArg;

#[derive(Args, Debug)]
pub struct ListRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
    #[arg(long)]
    offset: Option<u32>,
    #[arg(long)]
    count: Option<u32>,
}

impl ListRole {
    pub fn into_spec(self) -> spec::ListRole {
        spec::ListRole {
            governance_id: self.governance_id,
            role: self.role.into(),
            offset: self.offset,
            count: self.count,
        }
    }
}
