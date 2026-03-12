use near_crypto::PublicKey;
use near_fetch::ops::Function;
use near_sdk::{json_types::Base64VecU8, AccountId, NearToken};
use serde_json::json;
use templar_tools_common::{near::contract_version, version::RegistryVersion};

use crate::commands::SignerArgs;

#[derive(clap::Args, Debug)]
pub struct CreateProxyOracle {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    version_key: String,
    /// Name of the contract that will be deployed
    ///
    /// This will be used as the prefix for the account ID.
    #[arg(long)]
    name: String,
    /// Public keys to add as full access keys to the new account
    #[arg(long)]
    with_full_access_key: Vec<PublicKey>,
    /// Deposit to send with the deployment
    #[arg(long)]
    deposit: Option<NearToken>,
}

impl CreateProxyOracle {
    #[tracing::instrument(skip_all, name = "proxy_oracle_create", fields(registry_id = %self.registry_id, version_key = %self.version_key, name = %self.name))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
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

        tracing::info!("Creating proxy oracle from registry");

        let method = registry_version.deploy_method_name();
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.registry_id)
            .call(
                Function::new(method)
                    .deposit(deposit)
                    .max_gas()
                    .args_json(json!({
                        "name": self.name,
                        "version_key": self.version_key,
                        "init_args": Base64VecU8(b"{}".to_vec()),
                        "full_access_keys": self.with_full_access_key,
                    })),
            )
            .transact()
            .await?;

        tracing::info!("Proxy oracle created");

        Ok(())
    }
}
