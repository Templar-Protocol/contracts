use clap::Args;
use near_account_id::AccountId;
use near_api::PublicKey;
use templar_gateway_methods_spec::account as spec;
use templar_gateway_types::{ContractMethodName, NearToken};

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct AddKey {
    /// The key to add.
    #[arg(long, value_name = "PUBLIC_KEY")]
    key: PublicKey,
    /// Restrict the key to function calls on this contract. Omit for full access.
    #[arg(long, value_name = "ACCOUNT_ID")]
    receiver_id: Option<AccountId>,
    /// Methods the restricted key may call (repeatable). Omit to allow every method.
    #[arg(long, requires = "receiver_id", value_name = "METHOD")]
    method_name: Vec<ContractMethodName>,
    /// Gas budget of the restricted key. Omit for unlimited.
    #[arg(long, requires = "receiver_id", value_name = "AMOUNT")]
    allowance: Option<NearToken>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl AddKey {
    pub fn into_spec(self) -> spec::AddKey {
        spec::AddKey {
            public_key: self.key.into(),
            permission: match self.receiver_id {
                None => spec::AccessKeyPermission::FullAccess,
                Some(receiver_id) => spec::AccessKeyPermission::FunctionCall {
                    allowance: self.allowance,
                    receiver_id,
                    method_names: self.method_name,
                },
            },
        }
    }
}
