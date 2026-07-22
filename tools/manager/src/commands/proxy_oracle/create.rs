use anyhow::{bail, Context};
use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;
use templar_gateway_types::{version::ProxyOracleVersion, NearToken};

use crate::commands::full_access_key::FullAccessKeyArgs;
use crate::commands::signer::SignerArgs;

/// Deploy a proxy oracle from a registered version, granting the signer a full
/// access key so the operator retains control of the new account.
///
/// `--owner-id` seats the owner at init, so the oracle can be handed straight to
/// its governance contract instead of transferred afterwards.
#[derive(Args, Debug)]
pub struct Create {
    /// Registry that holds the version to deploy.
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    /// Sub-account name to create under the registry.
    #[arg(long, value_name = "NAME")]
    name: String,
    /// Version key of the proxy oracle contract in the registry.
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// Account that will own the oracle. Defaults to the deployer — the registry.
    #[arg(long, value_name = "ACCOUNT_ID")]
    owner_id: Option<AccountId>,
    #[command(flatten)]
    full_access_keys: FullAccessKeyArgs,
    /// Deposit funding the new account's storage and balance.
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

/// Refuse an `owner_id` the named version would ignore — the deploy would
/// otherwise succeed and leave the registry as owner.
///
/// Only as good as the version key, which is a convention the registry does not
/// enforce (see [`Version::from_version_key`]). An operator guardrail, hence the
/// CLI and not the gateway; ENG-463 replaces it with a check against the
/// contract's own ABI.
///
/// [`Version::from_version_key`]: templar_gateway_types::version::Version::from_version_key
fn check_owner_id_is_honored(version_key: &str, owner_id: &AccountId) -> anyhow::Result<()> {
    let version = ProxyOracleVersion::from_version_key(version_key)
        .with_context(|| format!("cannot tell whether {version_key} honors --owner-id"))?;

    if !version.new_accepts_owner_id() {
        bail!(
            "proxy oracle {version} cannot seat --owner-id {owner_id}: its `new` takes no \
             arguments, so the registry would own the oracle. Deploy >= 0.3.0."
        );
    }

    Ok(())
}

impl Create {
    pub fn try_into_spec(self) -> anyhow::Result<spec::Create> {
        if let Some(owner_id) = &self.owner_id {
            check_owner_id_is_honored(&self.version_key, owner_id)?;
        }

        let full_access_keys = self.full_access_keys.resolve(|| self.signer.public_key())?;

        Ok(spec::Create {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            owner_id: self.owner_id,
            full_access_keys: Some(full_access_keys),
            deposit: self.deposit,
        })
    }
}
