use near_sdk::AccountId;
use templar_common::market::MarketConfiguration;

use crate::{
    commands::{self, recover_nep141::RecoverNep141, SignerArgs},
    near, CliContext,
};

/// Remove a single market: recover NEP-141 tokens then delete the account.
#[derive(clap::Args, Debug)]
pub struct MarketRemove {
    #[command(flatten)]
    pub signer: SignerArgs,
    /// Recovered tokens will be sent to this account.
    #[arg(long)]
    pub beneficiary_id: AccountId,
    /// Proceed with account deletion even if prior actions (fetching
    /// configuration, recovering tokens, etc.) fail.
    #[arg(long)]
    pub force: bool,
}

impl MarketRemove {
    #[tracing::instrument(skip_all, name = "market_remove", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &CliContext) -> anyhow::Result<()> {
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
                    let recover_nep141 = RecoverNep141 {
                        signer: self.signer.clone(),
                        token_id: borrow_id,
                        beneficiary_id: self.beneficiary_id.clone(),
                    };
                    if let Err(error) = recover_nep141.run(ctx).await {
                        if !self.force {
                            return Err(error.context("Failed to recover borrow asset"));
                        }
                        tracing::warn!(%error, "Failed to recover borrow asset");
                    }
                }

                if let Some(collateral_id) = configuration.collateral_asset.into_nep141() {
                    let recover_nep141 = RecoverNep141 {
                        signer: self.signer.clone(),
                        token_id: collateral_id,
                        beneficiary_id: self.beneficiary_id.clone(),
                    };
                    if let Err(error) = recover_nep141.run(ctx).await {
                        if !self.force {
                            return Err(error.context("Failed to recover collateral asset"));
                        }
                        tracing::warn!(%error, "Failed to recover collateral asset");
                    }
                }
            }
            Err(error) => {
                if !self.force {
                    return Err(
                        anyhow::Error::new(error).context("Failed to fetch market configuration")
                    );
                }
                tracing::warn!(%error, "Failed to fetch market configuration");
            }
        }

        commands::delete_account(ctx, &self.signer, &self.beneficiary_id).await?;

        Ok(())
    }
}
