use anyhow::Context;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

/// Remove a single version from a registry.
///
/// Mirrors `script/ci/remove-version-from-registry.sh`.
#[derive(clap::Args, Debug)]
pub struct RemoveVersion {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long, env = "REGISTRY_ID")]
    registry_id: AccountId,
    #[arg(long)]
    version_key: String,
}

impl RemoveVersion {
    pub(crate) fn new(
        signer: super::SignerArgs,
        registry_id: AccountId,
        version_key: String,
    ) -> Self {
        Self {
            signer,
            registry_id,
            version_key,
        }
    }

    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        ctx.near
            .call(&self.signer.signer(), &self.registry_id, "remove_version")
            .args_json(json!({ "version_key": &self.version_key }))
            .deposit(ONE_YOCTO)
            .max_gas()
            .transact()
            .await
            .with_context(|| {
                format!(
                    "remove_version {} from {}",
                    self.version_key, self.registry_id
                )
            })?
            .into_result()
            .with_context(|| format!("remove_version execution: {}", self.version_key))?;

        tracing::info!("Version removed");
        Ok(())
    }
}
