use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::redstone as spec;

use super::RoleArg;

#[derive(Args, Debug)]
pub struct ListRole {
    /// RedStone adapter account to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Role whose members to list.
    #[arg(long, value_enum)]
    role: RoleArg,
}

impl ListRole {
    pub fn into_spec(self) -> spec::ListRole {
        spec::ListRole {
            oracle_id: self.oracle_id,
            role: self.role.into(),
        }
    }
}
