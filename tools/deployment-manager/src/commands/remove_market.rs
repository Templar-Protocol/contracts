use near_sdk::AccountId;
use templar_common::market::MarketConfiguration;

use crate::commands::recover_nep141::RecoverNep141;
use crate::near;

#[derive(clap::Args, Debug)]
pub struct RemoveMarket {
    #[command(flatten)]
    pub signer: super::SignerArgs,
    #[arg(long)]
    pub beneficiary_id: AccountId,
}

impl RemoveMarket {
    #[tracing::instrument(skip_all, name = "remove_market", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        if !near::account_exists(&ctx.near, &self.signer.account_id).await? {
            tracing::info!(account_id = %self.signer.account_id, "Account does not exist, nothing to do");
            return Ok(());
        }

        let configuration = ctx
            .near
            .view(&self.signer.account_id, "get_configuration")
            .await
            .and_then(|r| r.json::<MarketConfiguration>());

        match configuration {
            Ok(configuration) => {
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
            }
            Err(error) => {
                tracing::warn!(%error, "Failed to fetch market configuration");
            }
        }

        // Delete account
        let signer = self.signer.signer();
        ctx.batch(&signer, &self.signer.account_id)
            .delete_account(&self.beneficiary_id)
            .transact()
            .await?;

        Ok(())
    }
}
