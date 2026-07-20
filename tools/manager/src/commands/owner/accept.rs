use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::owner as spec;

use crate::commands::signer::SignerArgs;

/// Accept a pending ownership transfer.
#[derive(Args, Debug)]
pub struct Accept {
    /// Contract account.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Accept {
    pub fn into_spec(self) -> spec::AcceptOwner {
        spec::AcceptOwner {
            contract_id: self.contract_id,
        }
    }
}
