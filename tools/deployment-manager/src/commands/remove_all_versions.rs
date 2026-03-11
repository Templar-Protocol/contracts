use anyhow::Context;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

use super::remove_version::RemoveVersion;

/// Remove every version registered in a registry.
///
/// Mirrors `script/ci/remove-all-versions-from-registry.sh`.
#[derive(clap::Args, Debug)]
pub struct RemoveAllVersions {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long)]
    registry_id: AccountId,
}

impl RemoveAllVersions {
    pub(crate) fn new(signer: super::SignerArgs, registry_id: AccountId) -> Self {
        Self {
            signer,
            registry_id,
        }
    }

    #[tracing::instrument(skip_all, name = "remove_all_versions", fields(registry_id = %self.registry_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let versions: Vec<String> = ctx
            .near
            .view(&self.registry_id, "list_versions")
            .args_json(json!({}))
            .await
            .context("list_versions")?
            .json()
            .context("deserialise versions")?;

        tracing::info!(count = versions.len(), "Removing versions");

        for version_key in versions {
            tracing::info!(%version_key, "Removing version");
            if let Err(error) = RemoveVersion::new(
                self.signer.clone(),
                self.registry_id.clone(),
                version_key.clone(),
            )
            .run(ctx)
            .await
            {
                tracing::warn!(%version_key, %error, "Failed to remove version");
            }
        }

        Ok(())
    }
}
