use clap::Args;
use near_api::PublicKey;
use templar_gateway_methods_spec::account as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct DeleteKey {
    /// The key to remove.
    #[arg(long, value_name = "PUBLIC_KEY")]
    key: PublicKey,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl DeleteKey {
    pub fn into_spec(self) -> spec::DeleteKey {
        spec::DeleteKey {
            public_key: self.key.into(),
        }
    }
}
