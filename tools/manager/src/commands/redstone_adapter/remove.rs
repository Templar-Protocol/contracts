use near_sdk::AccountId;

use crate::commands::{self, SignerArgs};

#[derive(clap::Args, Debug)]
pub struct RedStoneAdapterRemove {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[arg(long)]
    pub beneficiary_id: AccountId,
}

impl RedStoneAdapterRemove {
    #[tracing::instrument(skip_all, name = "redstone_adapter_remove", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        commands::delete_account(ctx, &self.signer, &self.beneficiary_id).await?;
        Ok(())
    }
}
