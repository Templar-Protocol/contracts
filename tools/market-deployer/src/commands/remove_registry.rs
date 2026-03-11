use near_crypto::{InMemorySigner, SecretKey};
use near_sdk::AccountId;

use crate::near;

/// Remove all versions from a registry then delete its account.
///
/// Mirrors `script/ci/remove-registry.sh`.
pub async fn run(
    client: &near_fetch::Client,
    account_id: AccountId,
    secret_key: SecretKey,
    beneficiary_id: AccountId,
) -> anyhow::Result<()> {
    let registry_id = account_id.clone();

    if !near::account_exists(client, &registry_id).await? {
        tracing::info!(%registry_id, "Account does not exist, nothing to do");
        return Ok(());
    }

    // Remove all registered versions first.
    crate::commands::remove_all_versions::run(
        client,
        account_id.clone(),
        secret_key.clone(),
        registry_id.clone(),
    )
    .await?;

    let signer = InMemorySigner::from_secret_key(account_id, secret_key);
    tracing::info!(%registry_id, %beneficiary_id, "Deleting registry account");
    near::delete_account(client, &signer, &registry_id, &beneficiary_id).await?;

    Ok(())
}
