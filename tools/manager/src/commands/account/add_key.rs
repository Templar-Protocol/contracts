use clap::{ArgGroup, Args};
use near_account_id::AccountId;
use near_api::PublicKey;

use crate::commands::mpc::MpcContractArgs;
use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("key_source").args(["key", "mpc_dao"]).required(true)))]
pub struct AddKey {
    /// The key to add with full access.
    #[arg(long, value_name = "PUBLIC_KEY")]
    key: Option<PublicKey>,
    /// Instead, add the key the MPC derives for this DAO controlling the signer.
    #[arg(long, value_name = "ACCOUNT_ID")]
    mpc_dao: Option<AccountId>,
    /// Derivation path for `--mpc-dao`. Defaults to `<dao>-<signer>`.
    #[arg(long, requires = "mpc_dao", value_name = "PATH")]
    mpc_path: Option<String>,
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
}
