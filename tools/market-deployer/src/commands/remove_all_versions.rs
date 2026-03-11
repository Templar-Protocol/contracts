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
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
}

impl RemoveAllVersions {
    pub(crate) fn new(signer: super::SignerArgs, registry_id: AccountId) -> Self {
        Self { signer, registry_id }
    }

    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let versions: Vec<String> = ctx
            .near
            .view(&self.registry_id, "list_versions")
            .args_json(json!({}))
            .await
            .context("list_versions")?
            .json()
            .context("deserialise versions")?;

        tracing::info!(registry_id = %self.registry_id, count = versions.len(), "Removing versions");

        for version_key in versions {
            tracing::info!(registry_id = %self.registry_id, %version_key, "Removing version");
            RemoveVersion::new(
                self.signer.clone(),
                self.registry_id.clone(),
                version_key.clone(),
            )
            .run(ctx)
            .await
            .with_context(|| format!("remove version {version_key}"))?;
        }

        Ok(())
    }
}
