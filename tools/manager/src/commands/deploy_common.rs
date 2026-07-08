use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{Base64Bytes, NearToken};

use crate::commands::full_access_key::FullAccessKeyArgs;
use crate::context::CliContext;

/// Flags shared by every "deploy a registered version to a new sub-account"
/// command. Flatten this alongside the command's init-args-specific flags, then
/// call [`DeployCommonArgs::into_deploy`] with the built init bytes.
#[derive(Args, Debug)]
pub struct DeployCommonArgs {
    /// Registry that holds the version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry.
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the contract in the registry.
    #[arg(long, value_name = "KEY")]
    version_key: String,
    #[command(flatten)]
    full_access_keys: FullAccessKeyArgs,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl DeployCommonArgs {
    /// Build the registry deploy spec from these shared flags and the caller's
    /// init-args bytes, granting the operator's full access keys (resolved via
    /// `ctx`) so it retains control of the new account.
    pub fn into_deploy(self, ctx: &CliContext, init_args: Vec<u8>) -> anyhow::Result<spec::Deploy> {
        let full_access_keys = self.full_access_keys.resolve(ctx)?;
        Ok(spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_args),
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}
