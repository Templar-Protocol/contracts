use anyhow::Context;
use near_crypto::SecretKey;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

use crate::commands::market::remove::MarketRemove;
use crate::util::SignerArgs;
use crate::CliContext;

/// Remove all markets listed in a registry's deployments.
#[derive(clap::Args, Debug)]
pub struct ClearDeployments {
    /// Secret key authorized on all market accounts
    #[arg(long, env = "SECRET_KEY")]
    pub secret_key: SecretKey,
    /// Registry to query for the list of deployments
    #[arg(long)]
    pub registry_id: AccountId,
    /// Where to send recovered funds (defaults to registry ID)
    #[arg(long)]
    pub beneficiary_id: Option<AccountId>,
    /// Do not exit early if removing a market fails; sets --force on the
    /// individual market removal command.
    #[arg(long)]
    pub force: bool,
}

impl ClearDeployments {
    #[tracing::instrument(skip_all, name = "clear_deployments", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let beneficiary_id = self.beneficiary_id.as_ref().unwrap_or(&self.registry_id);
        tracing::info!(%beneficiary_id, "Clearing deployments for registry");

        let deployments: Vec<AccountId> = ctx
            .near
            .view(&self.registry_id, "list_deployments")
            .args_json(json!({}))
            .await
            .context("list_deployments")?
            .json()
            .context("deserialise deployments")?;

        tracing::info!(count = deployments.len(), "Removing markets");

        for market_id in deployments {
            tracing::info!(%market_id, "Removing market");
            let market_remove = MarketRemove {
                signer: SignerArgs {
                    account_id: market_id.clone(),
                    secret_key: self.secret_key.clone(),
                },
                beneficiary_id: beneficiary_id.clone(),
                force: self.force,
            };
            if let Err(error) = market_remove.run(ctx).await {
                if !self.force {
                    return Err(error);
                }
                tracing::warn!(%market_id, %error, "Failed to remove market");
            }
        }

        Ok(())
    }
}
