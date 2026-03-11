use anyhow::Context;
use near_fetch::ops::Function;
use near_primitives::views::FinalExecutionStatus;
use near_sdk::serde_json::json;

use super::FixedContractWasm;

const REGISTRY_PACKAGE: &str = "templar-registry-contract";

#[derive(clap::Args, Debug)]
pub struct DeployRegistry {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[command(flatten)]
    contract: FixedContractWasm,
    #[arg(long)]
    no_init: bool,
}

impl DeployRegistry {
    #[tracing::instrument(skip(context))]
    pub async fn run(self, context: &crate::CliContext) -> anyhow::Result<()> {
        let wasm = self.contract.wasm(context, REGISTRY_PACKAGE)?;

        let result = if self.no_init {
            context
                .near
                .batch(&self.signer.signer(), &self.signer.account_id)
                .deploy(&wasm)
                .transact()
                .await
                .context("deploy registry without init")?
        } else {
            context
                .near
                .batch(&self.signer.signer(), &self.signer.account_id)
                .deploy(&wasm)
                .call(Function::new("new").args_json(json!({})).max_gas())
                .transact()
                .await
                .context("deploy registry with init")?
        };

        tracing::info!(transaction_hash = %result.transaction.hash, "Deploy registry transaction submitted");

        // Ensure transaction was successful
        match result.status {
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                anyhow::bail!("Deploy registry failed: transaction not started");
            }
            FinalExecutionStatus::Failure(e) => {
                anyhow::bail!("Deploy registry failed: {e}");
            }
            FinalExecutionStatus::SuccessValue(_) => {}
        }

        tracing::info!("Registry deployed successfully");
        Ok(())
    }
}
