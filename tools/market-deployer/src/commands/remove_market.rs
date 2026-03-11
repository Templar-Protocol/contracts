use near_primitives::views::FinalExecutionStatus;
use near_sdk::AccountId;
use templar_common::market::MarketConfiguration;

use crate::commands::recover_nep141::RecoverNep141;
use crate::near;

#[derive(clap::Args, Debug)]
pub struct RemoveMarketArgs {
    #[command(flatten)]
    signer: super::SignerArgs,
    #[arg(long)]
    beneficiary_id: AccountId,
}

impl RemoveMarketArgs {
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        if !near::account_exists(&ctx.near, &self.signer.account_id).await? {
            tracing::info!(account_id = %self.signer.account_id, "Account does not exist, nothing to do");
            return Ok(());
        }

        let configuration = ctx
            .near
            .view(&self.signer.account_id, "get_configuration")
            .await?
            .json::<MarketConfiguration>()?;

        tracing::debug!(?configuration, "Market configuration");

        if let Some(borrow_id) = configuration.borrow_asset.into_nep141() {
            RecoverNep141 {
                signer: self.signer.clone(),
                token_id: borrow_id,
                beneficiary_id: self.beneficiary_id.clone(),
            }
            .run(ctx)
            .await?;
        }

        if let Some(collateral_id) = configuration.collateral_asset.into_nep141() {
            RecoverNep141 {
                signer: self.signer.clone(),
                token_id: collateral_id,
                beneficiary_id: self.beneficiary_id.clone(),
            }
            .run(ctx)
            .await?;
        }

        let e = ctx
            .near
            .batch(&self.signer.signer(), &self.signer.account_id)
            .delete_account(&self.beneficiary_id)
            .transact()
            .await?;

        match e.status {
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                // should never happen
                anyhow::bail!("Unexpected status: {:?}", e.status);
            }
            FinalExecutionStatus::Failure(tx_execution_error) => {
                anyhow::bail!("Transaction failed: {:?}", tx_execution_error);
            }
            FinalExecutionStatus::SuccessValue(_) => {
                // success
                Ok(())
            }
        }
    }
}
