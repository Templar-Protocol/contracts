use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::registry as spec;
use templar_gateway_types::{Base64Bytes, NearToken};

use crate::commands::full_access_key::FullAccessKeyArgs;
use crate::context::CliContext;

/// Deploy an already-registered contract version to a new account, granting full
/// access keys (the signer's by default) so the operator retains control.
#[derive(Args, Debug)]
pub struct Deploy {
    /// Registry that holds the version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry.
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the contract in the registry.
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// JSON file with the contract's init args (defaults to `null`).
    #[arg(long, value_name = "PATH")]
    init_args_file: Option<std::path::PathBuf>,
    #[command(flatten)]
    full_access_keys: FullAccessKeyArgs,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

impl Deploy {
    pub fn try_into_spec(self, ctx: &CliContext) -> anyhow::Result<spec::Deploy> {
        let full_access_keys = self.full_access_keys.resolve(ctx)?;

        let init_bytes = match self.init_args_file {
            Some(path) => std::fs::read(&path)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("read init args from {}", path.display()))?,
            None => b"null".to_vec(),
        };

        Ok(spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_bytes),
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}
