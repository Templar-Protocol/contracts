use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle_owner as spec;

#[derive(Args, Debug)]
pub struct ProposeOwner {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: Option<AccountId>,
}

impl ProposeOwner {
    pub fn into_spec(self) -> spec::ProposeOwner {
        spec::ProposeOwner {
            oracle_id: self.oracle_id,
            account_id: self.account_id,
        }
    }
}
