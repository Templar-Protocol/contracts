use near_fetch::ops::Function;
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};



#[derive(clap::Args, Debug)]
pub struct RecoverNep141 {
    #[command(flatten)]
    pub signer: super::SignerArgs,
    #[arg(long)]
    pub token_id: AccountId,
    #[arg(long)]
    pub beneficiary_id: AccountId,
}

impl RecoverNep141 {
    #[tracing::instrument(skip_all, name = "recover_nep141", fields(account_id = %self.signer.account_id, token_id = %self.token_id, beneficiary_id = %self.beneficiary_id))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        let signer = &self.signer.signer();

        // Transfer all tokens
        let balance = match ctx
            .near
            .view(&self.token_id, "ft_balance_of")
            .args_json(json!({ "account_id": signer.get_account_id() }))
            .await
            .and_then(|r| r.json::<U128>())
        {
            Ok(b) => b.0,
            Err(e) => {
                tracing::warn!(%self.token_id, error = %e, "Could not fetch FT balance, skipping transfer");
                0
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

        ctx.batch(signer, &self.token_id)
            .call(
                Function::new("storage_unregister")
                    .args_json(json!({ "force": true }))
                    .deposit(NearToken::from_yoctonear(1))
                    .max_gas(),
            )
            .transact()
            .await?;

        Ok(())
    }
}
