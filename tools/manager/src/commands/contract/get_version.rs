use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::contract as spec;

#[derive(Args, Debug)]
pub struct GetVersion {
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
}

impl GetVersion {
    pub fn into_spec(self) -> spec::GetVersion {
        spec::GetVersion {
            contract_id: self.contract_id,
        }
    }
}
