use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Remove {
    /// Recovered assets and the remaining balance are sent here.
    #[arg(long, value_name = "ACCOUNT_ID")]
    beneficiary_id: AccountId,
    /// Proceed with deletion even if reading the configuration or recovering an
    /// asset fails.
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Remove {
    pub fn beneficiary_id(&self) -> &AccountId {
        &self.beneficiary_id
    }

    pub fn force(&self) -> bool {
        self.force
    }
}
