use clap::Args;
use near_crypto::PublicKey;
use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, AccountId, NearToken};
use serde_json::json;
use templar_tools_common::{near::contract_version, version::RegistryVersion};

use crate::commands::SignerArgs;

/// Shared arguments for deploying a contract from a registry.
#[derive(Args, Debug)]
pub struct FromRegistry {
    #[arg(long)]
    pub registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    pub version_key: String,
    /// Name of the contract that will be deployed
    ///
    /// This will be used as the prefix for the account ID.
    #[arg(long)]
    pub name: String,
    /// Additional public keys to add as full access keys to the new account.
    /// The signer's public key is included by default unless --no-signer-full-access-key is set.
    #[arg(long)]
    pub with_full_access_key: Vec<PublicKey>,
    /// Do not add the signer's public key as a full access key on the new account
    #[arg(long)]
    pub no_signer_full_access_key: bool,
    /// Deposit to send with the deployment
    #[arg(long)]
    pub deposit: Option<NearToken>,
}

impl FromRegistry {
    pub fn new(registry_id: AccountId, version_key: String, name: String) -> Self {
        Self {
            registry_id,
            version_key,
            name,
            with_full_access_key: vec![],
            no_signer_full_access_key: false,
            deposit: None,
        }
    }

    pub fn with_deposit(mut self, deposit: NearToken) -> Self {
        self.deposit = Some(deposit);
        self
    }

    pub async fn run(
        &self,
        ctx: &crate::CliContext,
        signer_args: &SignerArgs,
        init_args: Vec<u8>,
    ) -> anyhow::Result<()> {
        let registry_version: RegistryVersion =
            contract_version(&ctx.near, &self.registry_id).await?;
        tracing::debug!(%registry_version);

        let deposit = self.deposit.unwrap_or_else(|| {
            if registry_version.supports_global_contracts() {
                NearToken::from_yoctonear(1)
            } else {
                NearToken::from_near(6)
            }
        });
        tracing::debug!(%deposit);

        let full_access_keys = {
            let mut keys = self.with_full_access_key.clone();
            if !self.no_signer_full_access_key {
                let signer_key = signer_args.secret_key.public_key();
                if !keys.contains(&signer_key) {
                    keys.push(signer_key);
                }
            }
            keys
        };
        tracing::debug!(?full_access_keys);

        let method = registry_version.deploy_method_name();

        ctx.batch(&signer_args.signer(), &self.registry_id)
            .call(
                Function::new(method)
                    .deposit(deposit)
                    .max_gas()
                    .args_json(json!({
                        "name": self.name,
                        "version_key": self.version_key,
                        "init_args": Base64VecU8(init_args),
                        "full_access_keys": full_access_keys,
                    })),
            )
            .transact()
            .await
    }
}
