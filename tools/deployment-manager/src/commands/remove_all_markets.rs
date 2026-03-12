use anyhow::Context;
use near_crypto::SecretKey;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

use crate::commands::SignerArgs;

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
    #[arg(long)]
    secret_key: SecretKey,
    /// Registry account containing the list of markets to remove.
    #[arg(long)]
    registry_id: AccountId,
    /// Beneficiary account to receive the NEAR balance from each removed
    /// market. If not provided, the registry account receives the balance.
    #[arg(long)]
    beneficiary_id: Option<AccountId>,
}

impl RemoveAllMarkets {
    #[tracing::instrument(skip_all, name = "remove_all_markets", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let deployments: Vec<AccountId> = ctx
            .near
            .view(&self.registry_id, "list_deployments")
            .args_json(json!({}))
            .await
            .context("list_deployments")?
            .json()
            .context("deserialise deployments")?;

        tracing::info!(count = deployments.len(), "Removing markets");

        let beneficiary_id = self.beneficiary_id.as_ref().unwrap_or(&self.registry_id);

        for market_id in deployments {
            tracing::info!(%market_id, "Removing market");
            if let Err(error) = (RemoveMarket {
                signer: SignerArgs {
                    account_id: market_id.clone(),
                    secret_key: self.secret_key.clone(),
                },
                beneficiary_id: beneficiary_id.clone(),
            })
            .run(ctx)
            .await
            {
                // Don't short-circuit because maybe the market was already removed.
                tracing::warn!(%market_id, %error, "Failed to remove market");
            }
        }

        Ok(())
    }
}
