use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::market as spec;

#[derive(Args, Debug)]
pub struct Get {
    /// Market to query.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: AccountId,
    /// Account whose static yield to read.
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl Get {
    pub fn into_spec(self) -> spec::GetStaticYield {
        spec::GetStaticYield {
            market_id: self.market_id,
            account_id: self.account_id,
        }
    }
}
