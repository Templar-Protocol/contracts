use anyhow::Context;
use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

use crate::commands::SignerArgs;
use crate::CliContext;

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

/// Remove one or all versions from a registry.
#[derive(clap::Args, Debug)]
pub struct VersionRemove {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    registry_id: AccountId,
    /// Remove all registered versions
    #[arg(long)]
    all: bool,
    /// Version key to remove (required without --all)
    #[arg(long)]
    version_key: Option<String>,
}

impl VersionRemove {
    #[tracing::instrument(skip_all, name = "version_remove", fields(registry_id = %self.registry_id, all = self.all))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        if self.all {
            remove_all(ctx, &self.signer, &self.registry_id).await
        } else {
            let version_key = self
                .version_key
                .as_deref()
                .context("--version-key is required when --all is not set")?;
            remove_one(ctx, &self.signer, &self.registry_id, version_key).await
        }
    }
}

/// Remove all versions from `registry_id`. Also called by `registry remove`.
pub(crate) async fn remove_all(
    ctx: &CliContext,
    signer: &SignerArgs,
    registry_id: &AccountId,
) -> anyhow::Result<()> {
    let versions: Vec<String> = ctx
        .near
        .view(registry_id, "list_versions")
        .args_json(json!({}))
        .await
        .context("list_versions")?
        .json()
        .context("deserialise versions")?;

    tracing::info!(count = versions.len(), %registry_id, "Removing versions");

    for version_key in versions {
        tracing::info!(%version_key, "Removing version");
        if let Err(error) = remove_one(ctx, signer, registry_id, &version_key).await {
            tracing::warn!(%version_key, %error, "Failed to remove version");
        }
    }

    Ok(())
}

async fn remove_one(
    ctx: &CliContext,
    signer: &SignerArgs,
    registry_id: &AccountId,
    version_key: &str,
) -> anyhow::Result<()> {
    let s = signer.signer();
    ctx.batch(&s, registry_id)
        .call(
            Function::new("remove_version")
                .args_json(json!({ "version_key": version_key }))
                .deposit(ONE_YOCTO)
                .max_gas(),
        )
        .transact()
        .await
        .with_context(|| format!("remove_version {version_key} from {registry_id}"))
}
