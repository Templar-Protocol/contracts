use near_sdk::AccountId;

use super::remove_all_versions::RemoveAllVersions;
use crate::near;

/// Remove all versions from a registry then delete its account.
///
/// Mirrors `script/ci/remove-registry.sh`.
#[derive(clap::Args, Debug)]
pub struct RemoveRegistry {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long)]
    beneficiary_id: AccountId,
}

impl RemoveRegistry {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let registry_id = self.signer.account_id.clone();

        if !near::account_exists(&ctx.near, &registry_id).await? {
            tracing::info!(%registry_id, "Account does not exist, nothing to do");
            return Ok(());
        }

        RemoveAllVersions::new(self.signer.clone(), registry_id.clone())
            .run(ctx)
            .await?;

        tracing::info!(%registry_id, beneficiary_id = %self.beneficiary_id, "Deleting registry account");
        near::delete_account(&ctx.near, &self.signer.signer(), &registry_id, &self.beneficiary_id)
            .await?;

        Ok(())
    }
}
