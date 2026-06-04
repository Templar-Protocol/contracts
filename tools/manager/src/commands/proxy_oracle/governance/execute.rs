use near_sdk::serde_json::json;
use near_sdk::AccountId;
use near_sdk::NearToken;
use templar_tools_common::near::Function;

use crate::util::SignerArgs;
use crate::CliContext;

pub async fn execute_proposal(
    ctx: &CliContext,
    signer: &SignerArgs,
    oracle_id: &AccountId,
    id: u32,
) -> anyhow::Result<()> {
    let signer = signer.signer();

    ctx.batch(&signer, oracle_id)
        .call(
            Function::new("execute_proposal")
                .args_json(json!({ "id": id }))?
                .deposit(NearToken::from_yoctonear(1))
                .max_gas(),
        )
        .transact()
        .await?;

    Ok(())
}

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
        execute_proposal(ctx, &self.signer, &self.oracle_id, self.id).await?;

        tracing::info!(id = self.id, "Proposal executed");
        Ok(())
    }
}
