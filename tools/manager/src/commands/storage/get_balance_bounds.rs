use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;

#[derive(Args, Debug)]
pub struct GetBalanceBounds {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
}

impl GetBalanceBounds {
    pub fn parse(self) -> spec::GetBalanceBounds {
        spec::GetBalanceBounds {
            contract_id: self.contract_id,
        }
    }
}
