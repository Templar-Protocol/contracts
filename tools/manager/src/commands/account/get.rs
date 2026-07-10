use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::account as spec;

#[derive(Args, Debug)]
pub struct Get {
    /// Account to read.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl Get {
    pub fn into_spec(self) -> spec::Get {
        spec::Get {
            account_id: self.account_id,
        }
    }
}
