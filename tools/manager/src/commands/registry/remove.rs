use clap::Args;
use near_account_id::AccountId;

#[derive(Args, Debug)]
pub struct Remove {
    /// Account to receive the registry account's remaining balance.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
}

impl Remove {
    pub fn beneficiary_id(&self) -> &AccountId {
        &self.beneficiary_id
    }
}
