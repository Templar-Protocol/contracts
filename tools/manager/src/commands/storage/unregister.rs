use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::storage as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct Unregister {
    /// Contract to unregister storage on.
    #[arg(long, value_name = "ACCOUNT_ID")]
    contract_id: AccountId,
    /// Force unregistration even with a non-zero balance.
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Unregister {
    pub fn into_spec(self) -> spec::Unregister {
        spec::Unregister {
            contract_id: self.contract_id,
            force: self.force,
        }
    }
}
