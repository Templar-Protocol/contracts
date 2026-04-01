use near_crypto::PublicKey;
use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, AccountId, NearToken};
use serde_json::json;
use templar_tools_common::{near::contract_version, version::RegistryVersion};

use crate::{
    util::{LoadArgs, SignerArgs},
    Runner,
};

use super::DeploymentSpec;

/// Shared arguments for deploying a contract from a registry.
#[derive(clap::Args)]
pub struct FromRegistry<C: DeploymentSpec> {
    /// Registry account ID to deploy from
    #[arg(long)]
    pub registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    pub version_key: String,
    /// Subaccount prefix for the deployed contract. The resulting account ID is `<name>.<registry_id>`.
    #[arg(long)]
    pub name: String,
    /// Additional public keys to add as full access keys to the new account.
    /// The signer's public key is included by default unless --no-signer-full-access-key is set.
    #[arg(long)]
    pub with_full_access_key: Vec<PublicKey>,
    /// Do not add the signer's public key as a full access key on the new account
    #[arg(long)]
    pub no_signer_full_access_key: bool,
    /// Deposit to send with the deployment. Defaults to 1 yoctoNEAR for registries that support
    /// global contracts, otherwise 6 NEAR.
    #[arg(long)]
    pub deposit: Option<NearToken>,

    #[command(flatten)]
    pub args: C::ArgsLoader,

    #[command(flatten)]
    pub signer: SignerArgs,
}

impl<C: DeploymentSpec> Runner<()> for FromRegistry<C> {
    type Output = ();

    async fn run(&self, ctx: &crate::CliContext, (): &()) -> anyhow::Result<Self::Output> {
        let signer = self.signer.signer();
        let registry_version: RegistryVersion =
            contract_version(&ctx.near, &self.registry_id).await?;
        tracing::debug!(%registry_version);

        let args = self.args.load_vec()?;

        let deposit = self.deposit.unwrap_or_else(|| {
            if registry_version.supports_global_contracts() {
                NearToken::from_yoctonear(1)
            } else {
                NearToken::from_near(6)
            }
        });
        tracing::debug!(%deposit);

        let full_access_keys = {
            let mut keys = Vec::with_capacity(self.with_full_access_key.len() + 1);
            if !self.no_signer_full_access_key {
                keys.push(signer.public_key());
            }
            for key in self.with_full_access_key.iter().cloned() {
                if keys.contains(&key) {
                    tracing::warn!("Duplicate full access key: {:?}", key);
                    continue;
                }
                keys.push(key);
            }
            keys
        };
        tracing::debug!(?full_access_keys);

        let method = registry_version.deploy_method_name();

        ctx.batch(&signer, &self.registry_id)
            .call(
                Function::new(method)
                    .deposit(deposit)
                    .max_gas()
                    .args_json(json!({
                        "name": self.name,
                        "version_key": self.version_key,
                        "init_args": Base64VecU8(args),
                        "full_access_keys": full_access_keys,
                    })),
            )
            .transact()
            .await
    }
}

impl<C: DeploymentSpec> FromRegistry<C> {
    pub fn new(
        registry_id: AccountId,
        version_key: String,
        name: String,
        args: C::ArgsLoader,
        signer: SignerArgs,
    ) -> Self {
        Self {
            registry_id,
            version_key,
            name,
            with_full_access_key: vec![],
            no_signer_full_access_key: false,
            deposit: None,
            args,
            signer,
        }
    }

    #[must_use]
    pub fn with_deposit(mut self, deposit: NearToken) -> Self {
        self.deposit = Some(deposit);
        self
    }
}
