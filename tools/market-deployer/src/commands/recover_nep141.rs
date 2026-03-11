use anyhow::Context;
use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

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
    #[tracing::instrument(skip(ctx))]
    pub async fn run(&self, ctx: &crate::CliContext) -> anyhow::Result<()> {
        transfer_all_tokens(
            &ctx.near,
            &self.signer.signer(),
            &self.token_id,
            &self.beneficiary_id,
        )
        .await;

        unregister_storage(&ctx.near, &self.signer.signer(), &self.token_id).await?;

        Ok(())
    }
}

/// Transfer the entire FT balance to `beneficiary_id`.  Errors are logged but
/// not propagated, matching the `set +e` behaviour in the shell script.
async fn transfer_all_tokens(
    client: &near_fetch::Client,
    signer: &Signer,
    token_id: &AccountId,
    beneficiary_id: &AccountId,
) {
    let balance = match client
        .view(token_id, "ft_balance_of")
        .args_json(json!({ "account_id": signer.get_account_id() }))
        .await
        .and_then(|r| r.json::<U128>())
    {
        Ok(b) => b.0,
        Err(e) => {
            tracing::warn!(%token_id, error = %e, "Could not fetch FT balance, skipping transfer");
            return;
        }
    };

    if balance == 0 {
        tracing::info!(%token_id, "Zero balance, skipping transfer");
        return;
    }

    tracing::info!(%token_id, %beneficiary_id, balance, "Transferring balance");

    if let Err(e) = client
        .call(signer, token_id, "ft_transfer")
        .args_json(json!({
            "receiver_id": beneficiary_id,
            "amount": U128(balance),
        }))
        .deposit(ONE_YOCTO)
        .transact()
        .await
    {
        tracing::warn!(%token_id, error = %e, "ft_transfer failed (ignoring)");
    }
}

/// Call `storage_unregister(force=true)` on the token contract.
async fn unregister_storage(
    client: &near_fetch::Client,
    signer: &Signer,
    token_id: &AccountId,
) -> anyhow::Result<()> {
    tracing::info!(%token_id, "Performing storage unregistration");

    client
        .call(signer, token_id, "storage_unregister")
        .args_json(json!({ "force": true }))
        .deposit(ONE_YOCTO)
        .max_gas()
        .transact()
        .await
        .with_context(|| format!("storage_unregister on {token_id}"))?
        .into_result()
        .with_context(|| format!("storage_unregister execution on {token_id}"))?;

    Ok(())
}
