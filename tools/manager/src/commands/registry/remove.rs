use near_sdk::AccountId;

use crate::{commands, util::SignerArgs};

/// Remove all versions from a registry then delete its account.
#[derive(clap::Args, Debug)]
pub struct RemoveRegistry {
    /// Signer for the deletion transaction. This same account is treated as the registry account.
    #[command(flatten)]
    pub signer: SignerArgs,
    /// Account to receive remaining funds when the registry account is deleted.
    #[arg(long)]
    pub beneficiary_id: AccountId,
}

impl RemoveRegistry {
    #[tracing::instrument(skip_all, name = "remove_registry", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let registry_id = self.signer.account_id.clone();

        if !crate::near::account_exists(&ctx.near, &registry_id).await? {
            tracing::info!(%registry_id, "Account does not exist, nothing to do");
            return Ok(());
        }

        super::version::remove::remove_all(ctx, &self.signer, &registry_id).await?;

        tracing::info!(%registry_id, beneficiary_id = %self.beneficiary_id, "Deleting registry account");
        commands::delete_account(ctx, &self.signer, &self.beneficiary_id).await?;

        Ok(())
    }
}
