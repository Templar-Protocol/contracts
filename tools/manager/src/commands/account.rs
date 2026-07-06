use clap::{Args, Subcommand};
use near_account_id::AccountId;
use templar_gateway_methods_spec::account as spec;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum AccountNs {
    Get(Get),
}

#[derive(Args, Debug)]
pub struct Get {
    #[arg(long, value_name = "ACCOUNT_ID")]
    account_id: AccountId,
}

impl Get {
    pub fn parse(self) -> spec::Get {
        spec::Get {
            account_id: self.account_id,
        }
    }
}
