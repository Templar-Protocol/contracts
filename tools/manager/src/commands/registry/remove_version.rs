use clap::{ArgGroup, Args};
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("which_version").args(["version_key", "all"]).required(true)
))]
pub struct RemoveVersion {
    /// Registry to remove the version from.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Version key to remove. Omit and pass `--all` to remove every version.
    #[arg(long, value_name = "KEY")]
    version_key: Option<String>,
    /// Remove every version currently in the registry.
    #[arg(long, conflicts_with = "print")]
    all: bool,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl RemoveVersion {
    pub fn registry_id(&self) -> &AccountId {
        &self.registry_id
    }

    /// The single-version spec, or `None` when `--all` was requested (the
    /// dispatcher then lists and removes each version).
    pub fn single(&self) -> Option<spec::RemoveVersion> {
        self.version_key
            .clone()
            .map(|version_key| spec::RemoveVersion {
                registry_id: self.registry_id.clone(),
                version_key,
            })
    }
}
