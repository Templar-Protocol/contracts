use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;

#[derive(Args, Debug)]
pub struct GetBalanceOf {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl GetBalanceOf {
    pub fn into_spec(self) -> spec::GetBalanceOf {
        spec::GetBalanceOf {
            contract_id: self.contract_id,
            account_id: self.account_id,
        }
    }
}
