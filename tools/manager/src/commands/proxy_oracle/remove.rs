use near_sdk::AccountId;

use crate::commands;
use crate::util::SignerArgs;

#[derive(clap::Args, Debug)]
pub struct ProxyOracleRemove {
    /// Signer for the deletion transaction. This same account is deleted.
    #[command(flatten)]
    pub signer: SignerArgs,
    /// Account to receive remaining funds when the proxy oracle account is deleted
    #[arg(long)]
    pub beneficiary_id: AccountId,
}

impl ProxyOracleRemove {
    #[tracing::instrument(skip_all, name = "proxy_oracle_remove", fields(signer_id = %self.signer.signer_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        commands::delete_account(ctx, &self.signer, &self.beneficiary_id).await?;
        Ok(())
    }
}
