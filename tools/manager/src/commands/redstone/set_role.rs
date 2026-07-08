use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::redstone as spec;

use super::RoleArg;

#[derive(Args, Debug)]
pub struct SetRole {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
    #[arg(long, value_enum)]
    role: RoleArg,
    /// Revoke the role instead of granting it.
    #[arg(long)]
    revoke: bool,
}

impl SetRole {
    pub fn into_spec(self) -> spec::SetRole {
        spec::SetRole {
            oracle_id: self.oracle_id,
            account_id: self.account_id,
            role: self.role.into(),
            set: !self.revoke,
        }
    }
}
