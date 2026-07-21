use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::owner as spec;

use crate::commands::signer::SignerArgs;

/// Renounce contract ownership.
#[derive(Args, Debug)]
pub struct Renounce {
    /// Contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Renounce {
    pub fn into_spec(self) -> spec::RenounceOwner {
        spec::RenounceOwner {
            contract_id: self.contract_id,
        }
    }
}
