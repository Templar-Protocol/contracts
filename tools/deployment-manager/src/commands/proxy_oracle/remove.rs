use near_sdk::AccountId;

use crate::{near, CliContext};

use crate::commands::SignerArgs;

#[derive(clap::Args, Debug)]
pub struct ProxyOracleRemove {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    beneficiary_id: AccountId,
}

impl ProxyOracleRemove {
    #[tracing::instrument(skip_all, name = "proxy_oracle_remove", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        if !near::account_exists(&ctx.near, &self.signer.account_id).await? {
            tracing::info!(account_id = %self.signer.account_id, "Account does not exist, nothing to do");
            return Ok(());
        }

        let signer = self.signer.signer();
        ctx.batch(&signer, &self.signer.account_id)
            .delete_account(&self.beneficiary_id)
            .transact()
            .await?;

        Ok(())
    }
}
