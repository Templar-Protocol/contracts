use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;

#[derive(Args, Debug)]
pub struct GetRoles {
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetRoles {
    pub fn parse(self) -> spec::GetRoles {
        spec::GetRoles {
            governance_id: self.governance_id,
            account_id: self.account_id,
        }
    }
}
