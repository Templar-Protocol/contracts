use near_sdk::AccountId;

use crate::util::SignerArgs;

pub mod deployment;
pub mod market;
pub mod proxy_oracle;
pub mod recover_nep141;
pub mod redstone_adapter;
pub mod registry;
pub mod storage_deposit;

/// Check if the account exists and, if so, delete it and send remaining funds
/// to `beneficiary_id`. Returns `Ok(false)` if the account did not exist.
pub async fn delete_account(
    ctx: &crate::CliContext,
    signer: &SignerArgs,
    beneficiary_id: &AccountId,
) -> anyhow::Result<bool> {
    if !crate::near::account_exists(&ctx.near, &signer.account_id).await? {
        tracing::info!(account_id = %signer.account_id, "Account does not exist, nothing to do");
        return Ok(false);
    }

    let s = signer.signer();
    ctx.batch(&s, &signer.account_id)
        .delete_account(beneficiary_id)
        .transact()
        .await?;

    Ok(true)
}
