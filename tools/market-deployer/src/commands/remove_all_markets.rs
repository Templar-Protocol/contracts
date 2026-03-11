use anyhow::Context;
use near_crypto::SecretKey;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

#[derive(clap::Args)]
pub struct RemoveAllMarkets {
    /// Secret key authorised on all market accounts
    #[arg(long)]
    secret_key: SecretKey,
    #[arg(long)]
    registry_id: AccountId,
}

impl RemoveAllMarkets {
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        run(client, self.secret_key.clone(), self.registry_id.clone()).await
    }
}

/// Remove every market listed in a registry's deployments.
///
/// The same `secret_key` must be authorised on every market account.
///
/// Mirrors `script/ci/remove-all-markets-from-registry.sh`.
pub async fn run(
    client: &near_fetch::Client,
    secret_key: SecretKey,
    registry_id: AccountId,
) -> anyhow::Result<()> {
    let deployments: Vec<AccountId> = client
        .view(&registry_id, "list_deployments")
        .args_json(json!({}))
        .await
        .context("list_deployments")?
        .json()
        .context("deserialise deployments")?;

    tracing::info!(%registry_id, count = deployments.len(), "Removing markets");

    for market_id in deployments {
        tracing::info!(%market_id, "Removing market");

        // The beneficiary for each removed market is the registry.
        crate::commands::remove_market::run(
            client,
            market_id.clone(),
            secret_key.clone(),
            registry_id.clone(),
        )
        .await
        .with_context(|| format!("remove market {market_id}"))?;
    }

    Ok(())
}
