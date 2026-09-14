use clap::{ArgGroup, Args};
use near_account_id::AccountId;
use near_api::PublicKey;
use templar_gateway_methods_spec::account as spec;
use templar_gateway_types::{ContractMethodName, NearToken};

use crate::commands::mpc::MpcContractArgs;
use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("key_source").args(["key", "mpc_dao"]).required(true)))]
pub struct AddKey {
    /// The key to add.
    #[arg(long, value_name = "PUBLIC_KEY")]
    key: Option<PublicKey>,
    /// Instead, add the key the MPC derives for this DAO controlling the signer.
    #[arg(long, value_name = "ACCOUNT_ID")]
    mpc_dao: Option<AccountId>,
    /// Derivation path for `--mpc-dao`. Defaults to `<dao>-<signer>`.
    #[arg(long, requires = "mpc_dao", value_name = "PATH")]
    mpc_path: Option<String>,
    /// Restrict the key to function calls on this contract. Omit for full access.
    #[arg(long, value_name = "ACCOUNT_ID")]
    receiver_id: Option<AccountId>,
    /// Methods the restricted key may call (repeatable). Omit to allow every method.
    #[arg(long, requires = "receiver_id", value_name = "METHOD")]
    method_name: Vec<String>,
    /// Gas budget of the restricted key. Omit for unlimited.
    #[arg(long, requires = "receiver_id", value_name = "AMOUNT")]
    allowance: Option<NearToken>,
    #[command(flatten)]
    pub mpc: MpcContractArgs,
    #[command(flatten)]
    pub signer: SignerArgs,
}

/// Which MPC key to derive, when `--mpc-dao` was given.
pub struct Derivation {
    pub dao: AccountId,
    pub path: String,
}

impl AddKey {
    pub fn derivation(&self) -> Option<Derivation> {
        let dao = self.mpc_dao.clone()?;
        let path = self
            .mpc_path
            .clone()
            .unwrap_or_else(|| format!("{dao}-{}", *self.signer.account_id()));
        Some(Derivation { dao, path })
    }

    /// The literal `--key`, when no derivation was requested.
    pub fn literal_public_key(&self) -> Option<PublicKey> {
        self.key
    }

    pub fn permission(&self) -> spec::AccessKeyPermission {
        match &self.receiver_id {
            None => spec::AccessKeyPermission::FullAccess,
            Some(receiver_id) => spec::AccessKeyPermission::FunctionCall {
                allowance: self.allowance,
                receiver_id: receiver_id.clone(),
                method_names: self
                    .method_name
                    .iter()
                    .cloned()
                    .map(ContractMethodName::from)
                    .collect(),
            },
        }
    }
}

#[derive(Args, Debug)]
pub struct DeleteKey {
    /// The key to remove.
    #[arg(long, value_name = "PUBLIC_KEY")]
    key: PublicKey,
    #[command(flatten)]
    pub signer: SignerArgs,
}

impl DeleteKey {
    pub fn into_spec(self) -> spec::DeleteKey {
        spec::DeleteKey {
            public_key: self.key.into(),
        }
    }
}
