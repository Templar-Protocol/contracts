use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::owner as spec;

/// Read the current contract owner.
#[derive(Args, Debug)]
pub struct Get {
    /// Contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
}

impl Get {
    pub fn into_spec(self) -> spec::GetOwner {
        spec::GetOwner {
            contract_id: self.contract_id,
        }
    }
}
