use near_crypto::PublicKey;
use near_sdk::{json_types::Base64VecU8, AccountId, NearToken};
use serde_json::json;
use templar_tools_common::{near::contract_version, version::RegistryVersion};

#[derive(clap::Args, Debug)]
pub struct DeployFromRegistry {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    /// Version key to deploy from the registry
    #[arg(long)]
    version_key: String,
    /// JSON-encoded init args to pass to the deployed contract
    #[arg(long)]
    init_args: serde_json::Value,
    /// Name of the contract that will be deployed
    ///
    /// This will be used as the prefix for the account ID.
    #[arg(long)]
    name: String,
    /// Public keys to add as full access keys to the new account
    #[arg(long)]
    with_full_access_key: Vec<PublicKey>,
}

impl DeployFromRegistry {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let registry_version: RegistryVersion =
            contract_version(&ctx.near, &self.registry_id).await?;

        let deposit = if registry_version.supports_global_contracts() {
            NearToken::from_yoctonear(1)
        } else {
            NearToken::from_near(6)
        };

        let init_args = serde_json::to_vec(&self.init_args)?;

        tracing::info!(%deposit, "Deploying from registry");

        let method = registry_version.deploy_method_name();
        let signer = self.signer.signer();

        ctx.near
            .call(&signer, &self.registry_id, method)
            .deposit(deposit)
            .max_gas()
            .args_json(json!({
                "name": self.name,
                "version_key": self.version_key,
                "init_args": Base64VecU8(init_args),
                "full_access_keys": self.with_full_access_key,
            }))
            .transact()
            .await?;

        tracing::info!("Deployed from registry");

        Ok(())
    }
}
