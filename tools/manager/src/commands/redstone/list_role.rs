use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::redstone as spec;

use super::RoleArg;

#[derive(Args, Debug)]
pub struct ListRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
}

impl ListRole {
    pub fn parse(self) -> spec::ListRole {
        spec::ListRole {
            oracle_id: self.oracle_id,
            role: self.role.into(),
        }
    }
}
