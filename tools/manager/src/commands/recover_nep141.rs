use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, AccountIdRef, NearToken};
use templar_tools_common::near::{self, Function};

#[derive(clap::Args, Debug)]
pub struct RecoverNep141 {
    /// Signer for the recovery transaction. Tokens are recovered from this same account.
    #[command(flatten)]
    pub signer: super::SignerArgs,
    /// Token ID to recover
    #[arg(long)]
    pub token_id: AccountId,
    /// Beneficiary account ID to receive the tokens
    #[arg(long)]
    pub beneficiary_id: AccountId,
    /// Forward `force` to `storage_unregister`
    ///
    /// This only affects the `storage_unregister(force=...)` call during storage unregistration.
    /// It does not skip transfer attempts and does not allow unregistering here with a non-zero balance.
    #[arg(long)]
    pub force: bool,
}

async fn ft_balance_of(
    near: &near::Client,
    token_id: &AccountId,
    account_id: &AccountIdRef,
) -> anyhow::Result<u128> {
    Ok(near::view::<U128>(
        near,
        token_id,
        "ft_balance_of",
        json!({ "account_id": account_id }),
    )
    .await?
    .0)
}

impl RecoverNep141 {
    #[tracing::instrument(skip_all, name = "recover_nep141", fields(account_id = %self.signer.account_id, token_id = %self.token_id, beneficiary_id = %self.beneficiary_id, force = self.force))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let signer = self.signer.signer();

        // Transfer all tokens
        let balance = match ft_balance_of(
            &ctx.near,
            &self.token_id,
            signer.get_account_id().as_ref(),
        )
        .await
        {
            Ok(b) => b,
            Err(e) => {
                anyhow::bail!("Could not fetch FT balance, skipping transfer: {e}")
            }
        };

        if balance > 0 {
            tracing::info!(%self.token_id, %self.beneficiary_id, balance, "Transferring balance");

            if let Err(error) = ctx
                .batch(&signer, &self.token_id)
                .call(
                    Function::new("ft_transfer")
                        .args_json(json!({
                            "receiver_id": &self.beneficiary_id,
                            "amount": U128(balance),
                        }))?
                        .deposit(NearToken::from_yoctonear(1)),
                )
                .transact()
                .await
            {
                tracing::warn!(%self.token_id, %error, "ft_transfer failed; --force only affects storage_unregister");
            }
        } else {
            tracing::info!(%self.token_id, "Zero balance, skipping transfer");
        }

        // Unregister storage
        tracing::info!(%self.token_id, "Performing storage unregistration");

        // Read balance again, unregister storage if balance is zero
        let balance = match ft_balance_of(
            &ctx.near,
            &self.token_id,
            signer.get_account_id().as_ref(),
        )
        .await
        {
            Ok(b) => b,
            Err(e) => {
                anyhow::bail!("Failed to fetch balance before storage unregistration: {e}")
            }
        };

        if balance == 0 {
            tracing::info!(force = self.force, "Balance is zero, unregistering storage");
            ctx.batch(&signer, &self.token_id)
                .call(
                    Function::new("storage_unregister")
                        .args_json(json!({ "force": self.force }))?
                        .deposit(NearToken::from_yoctonear(1))
                        .max_gas(),
                )
                .transact()
                .await?;
            Ok(())
        } else {
            anyhow::bail!("Non-zero balance ({balance}) after attempting to transfer all to beneficiary; --force only affects storage_unregister and this command will still skip storage unregistration")
        }
    }
}
