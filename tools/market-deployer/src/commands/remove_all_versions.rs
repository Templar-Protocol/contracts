use anyhow::Context;
use near_crypto::SecretKey;
use near_sdk::serde_json::json;
use near_sdk::AccountId;

/// Remove every version registered in a registry.
///
/// Mirrors `script/ci/remove-all-versions-from-registry.sh`.
pub async fn run(
    client: &near_fetch::Client,
    account_id: AccountId,
    secret_key: SecretKey,
    registry_id: AccountId,
) -> anyhow::Result<()> {
    let versions: Vec<String> = client
        .view(&registry_id, "list_versions")
        .args_json(json!({}))
        .await
        .context("list_versions")?
        .json()
        .context("deserialise versions")?;

    tracing::info!(%registry_id, count = versions.len(), "Removing versions");

    for version_key in versions {
        tracing::info!(%registry_id, version_key, "Removing version");

        crate::commands::remove_version::run(
            client,
            account_id.clone(),
            secret_key.clone(),
            registry_id.clone(),
            &version_key,
        )
        .await
        .with_context(|| format!("remove version {version_key}"))?;
    }

    Ok(())
}
