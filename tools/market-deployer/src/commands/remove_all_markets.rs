use anyhow::Context;
use near_crypto::SecretKey;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

use super::remove_market::RemoveMarket;

/// Remove every market listed in a registry's deployments.
///
/// The same `secret_key` must be authorised on every market account.
/// The registry account receives the NEAR balance from each removed market.
///
/// Mirrors `script/ci/remove-all-markets-from-registry.sh`.
#[derive(clap::Args, Debug)]
pub struct RemoveAllMarkets {
    /// Secret key authorised on all market accounts
    #[arg(long, env = "SECRET_KEY")]
    secret_key: SecretKey,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
}

impl RemoveAllMarkets {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let deployments: Vec<AccountId> = ctx
            .near
            .view(&self.registry_id, "list_deployments")
            .args_json(json!({}))
            .await
            .context("list_deployments")?
            .json()
            .context("deserialise deployments")?;

        tracing::info!(registry_id = %self.registry_id, count = deployments.len(), "Removing markets");

        for market_id in deployments {
            tracing::info!(%market_id, "Removing market");
            RemoveMarket::new(
                market_id.clone(),
                self.secret_key.clone(),
                self.registry_id.clone(),
            )
            .run(ctx)
            .await
            .with_context(|| format!("remove market {market_id}"))?;
        }

        Ok(())
    }
}
