use anyhow::Context;
use near_crypto::{InMemorySigner, SecretKey};
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

/// Remove a single version from a registry.
///
/// Mirrors `script/ci/remove-version-from-registry.sh`.
pub async fn run(
    client: &near_fetch::Client,
    account_id: AccountId,
    secret_key: SecretKey,
    registry_id: AccountId,
    version_key: &str,
) -> anyhow::Result<()> {
    tracing::info!(%registry_id, version_key, "Removing version");

    let signer = InMemorySigner::from_secret_key(account_id, secret_key);

    client
        .call(&signer, &registry_id, "remove_version")
        .args_json(json!({ "version_key": version_key }))
        .deposit(ONE_YOCTO)
        .max_gas()
        .transact()
        .await
        .with_context(|| format!("remove_version {version_key} from {registry_id}"))?
        .into_result()
        .with_context(|| format!("remove_version execution: {version_key}"))?;

    tracing::info!(%registry_id, version_key, "Version removed");
    Ok(())
}
