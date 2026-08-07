use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{primitive::PublicKey, NearToken};

use crate::commands::full_access_key::FullAccessKeyArgs;

/// Flags shared by every "deploy a registered version to a new sub-account"
/// command. Flatten this alongside the command's own init-args flags.
#[derive(Args, Debug)]
pub struct DeployTargetArgs {
    /// Registry that holds the version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry (e.g. `usdc-near`).
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the contract in the registry. Readable so the commands whose
    /// contract has version-gated init args can check it before deploying.
    #[arg(long, value_name = "KEY")]
    pub(crate) version_key: String,
    #[command(flatten)]
    full_access_keys: FullAccessKeyArgs,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl DeployTargetArgs {
    /// Resolve these flags into the target every deploy-from-registry method
    /// shares, granting the operator's full access keys (the signer's
    /// `signer_public_key` by default) so it retains control of the new account.
    pub fn resolve(
        self,
        signer_public_key: impl FnOnce() -> anyhow::Result<PublicKey>,
    ) -> anyhow::Result<spec::DeployTarget> {
        let full_access_keys = self.full_access_keys.resolve(signer_public_key)?;
        Ok(spec::DeployTarget {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}
