use clap::{Args, Subcommand};
use near_account_id::AccountId;
use templar_gateway_methods_spec::account as spec;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum AccountNs {
    Get(Get),
    /// Delete the signer account, sweeping its balance to a beneficiary.
    Delete(Delete),
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

#[derive(Args, Debug)]
pub struct Delete {
    /// Account to receive the deleted account's remaining balance.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
}

impl Delete {
    pub fn parse(self) -> spec::Delete {
        spec::Delete {
            beneficiary_id: self.beneficiary_id,
        }
    }
}
