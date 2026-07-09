use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Remove {
    /// Account to receive the registry account's remaining balance.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Remove {
    pub fn beneficiary_id(&self) -> &AccountId {
        &self.beneficiary_id
    }
}
