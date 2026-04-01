use near_sdk::AccountId;
use templar_common::{
    asset::{AssetClass, FungibleAsset},
    market::MarketConfiguration,
};

use crate::{
    commands::{self, recover_nep141::RecoverNep141},
    near,
    util::SignerArgs,
    CliContext,
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
    /// configuration, recovering tokens, etc.) fail. Forwards --force to the
    /// underlying NEP-141 token recovery command.
    #[arg(long)]
    pub force: bool,
}

impl MarketRemove {
    #[tracing::instrument(skip_all, name = "market_remove", fields(account_id = %self.signer.account_id, beneficiary_id = %self.beneficiary_id, force = self.force))]
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
            Ok(c) => {
                tracing::debug!(market_configuration = ?c);

                self.try_recover(ctx, c.borrow_asset).await?;
                self.try_recover(ctx, c.collateral_asset).await?;
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

    async fn try_recover<T: AssetClass>(
        &self,
        ctx: &CliContext,
        asset: FungibleAsset<T>,
    ) -> anyhow::Result<()> {
        if let Some(token_id) = asset.into_nep141() {
            let recover = RecoverNep141 {
                signer: self.signer.clone(),
                token_id: token_id.clone(),
                beneficiary_id: self.beneficiary_id.clone(),
                force: self.force,
            };
            if let Err(error) = recover.run(ctx).await {
                if !self.force {
                    anyhow::bail!("Failed to recover asset {token_id}: {error}");
                }
                tracing::warn!(%token_id, %error, "Failed to recover asset");
            }
        }

        Ok(())
    }
}
