use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::account as spec;

#[derive(Args, Debug)]
pub struct Delete {
    /// Account to receive the deleted account's remaining balance.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
}

impl Delete {
    pub fn into_spec(self) -> spec::Delete {
        spec::Delete {
            beneficiary_id: self.beneficiary_id,
        }
    }
}
