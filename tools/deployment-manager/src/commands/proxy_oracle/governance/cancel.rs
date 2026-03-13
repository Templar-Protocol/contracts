use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;

use crate::commands::SignerArgs;
use crate::CliContext;



#[derive(clap::Args, Debug)]
pub struct CancelProposal {
    #[command(flatten)]
    signer: SignerArgs,
    #[arg(long)]
    oracle_id: AccountId,
    /// Proposal ID to cancel
    #[arg(long)]
    id: u32,
}

impl CancelProposal {
    #[tracing::instrument(skip_all, name = "governance_cancel", fields(oracle_id = %self.oracle_id, id = self.id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.oracle_id)
            .call(
                Function::new("gov_cancel")
                    .args_json(json!({ "id": self.id }))
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!(id = self.id, "Proposal cancelled");
        Ok(())
    }
}
