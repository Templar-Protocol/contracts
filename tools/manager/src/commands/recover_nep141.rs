use near_fetch::ops::Function;
use near_fetch::signer::ExposeAccountId;
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, AccountIdRef, NearToken};

#[derive(clap::Args, Debug)]
pub struct RecoverNep141 {
    #[command(flatten)]
    pub signer: super::SignerArgs,
    /// Token ID to recover
    #[arg(long)]
    pub token_id: AccountId,
    /// Beneficiary account ID to receive the tokens
    #[arg(long)]
    pub beneficiary_id: AccountId,
    /// Force-unregister from storage
    ///
    /// Unregisters even if recovering tokens fails or balance is non-zero even after sending.
    #[arg(long)]
    pub force: bool,
}

async fn ft_balance_of(
    near: &near_fetch::Client,
    token_id: &AccountId,
    account_id: &AccountIdRef,
) -> Result<u128, near_fetch::Error> {
    Ok(near
        .view(token_id, "ft_balance_of")
        .args_json(json!({ "account_id": account_id }))
        .await?
        .json::<U128>()?
        .0)
}

impl RecoverNep141 {
    #[tracing::instrument(skip_all, name = "recover_nep141", fields(account_id = %self.signer.account_id, token_id = %self.token_id, beneficiary_id = %self.beneficiary_id, force = self.force))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let signer = &self.signer.signer();

        // Transfer all tokens
        let balance = match ft_balance_of(&ctx.near, &self.token_id, signer.account_id()).await {
            Ok(b) => b,
            Err(e) => {
                anyhow::bail!("Could not fetch FT balance, skipping transfer: {e}")
            }
        };

        if balance > 0 {
            tracing::info!(%self.token_id, %self.beneficiary_id, balance, "Transferring balance");

            if let Err(error) = ctx
                .batch(signer, &self.token_id)
                .call(
                    Function::new("ft_transfer")
                        .args_json(json!({
                            "receiver_id": &self.beneficiary_id,
                            "amount": U128(balance),
                        }))
                        .deposit(NearToken::from_yoctonear(1)),
                )
                .transact()
                .await
            {
                tracing::warn!(%self.token_id, %error, "ft_transfer failed (ignoring)");
            }
        } else {
            tracing::info!(%self.token_id, "Zero balance, skipping transfer");
        }

        // Unregister storage
        tracing::info!(%self.token_id, "Performing storage unregistration");

        // Read balance again, unregister storage if balance is zero
        let balance = match ft_balance_of(&ctx.near, &self.token_id, signer.account_id()).await {
            Ok(b) => b,
            Err(e) => {
                anyhow::bail!("Failed to fetch balance before storage registration: {e}")
            }
        };

        if balance == 0 {
            tracing::info!("Balance is zero, unregistering storage");
            ctx.batch(signer, &self.token_id)
                .call(
                    Function::new("storage_unregister")
                        .args_json(json!({ "force": self.force }))
                        .deposit(NearToken::from_yoctonear(1))
                        .max_gas(),
                )
                .transact()
                .await?;
            Ok(())
        } else {
            anyhow::bail!("Non-zero balance ({balance}) after attempting to transfer all to beneficiary, skipping storage unregistration")
        }
    }
}
