use near_fetch::ops::Function;
use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;

use crate::util::SignerArgs;
use crate::CliContext;

#[derive(clap::Args, Debug)]
pub struct ExecuteProposal {
    #[command(flatten)]
    pub signer: SignerArgs,
    #[arg(long)]
    pub oracle_id: AccountId,
    /// Proposal ID to execute
    #[arg(long)]
    pub id: u32,
}

impl ExecuteProposal {
    #[tracing::instrument(skip_all, name = "governance_execute", fields(oracle_id = %self.oracle_id, id = self.id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
        let signer = self.signer.signer();

        ctx.batch(&signer, &self.oracle_id)
            .call(
                Function::new("gov_execute")
                    .args_json(json!({ "id": self.id }))
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        tracing::info!(id = self.id, "Proposal executed");
        Ok(())
    }
}
