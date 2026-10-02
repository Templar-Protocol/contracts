use clap::Args;
use near_account_id::AccountId;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Upgrade {
    /// Registry to upgrade; it also signs, since only it can replace its own code.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Catalogued registry release to upgrade to. Defaults to the newest.
    #[arg(long, value_name = "VERSION")]
    release: Option<String>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Upgrade {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    pub fn release(&self) -> Option<&str> {
        self.release.as_deref()
    }
}
