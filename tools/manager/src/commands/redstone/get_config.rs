use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::redstone as spec;

#[derive(Args, Debug)]
pub struct GetConfig {
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
}

impl GetConfig {
    pub fn parse(self) -> spec::GetConfig {
        spec::GetConfig {
            oracle_id: self.oracle_id,
        }
    }
}
