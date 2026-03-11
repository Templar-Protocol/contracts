use anyhow::Context;
use near_crypto::{InMemorySigner, SecretKey};
use near_sdk::serde_json::json;
use near_sdk::{AccountId, NearToken};

/// `0.00125 NEAR` expressed in yoctoNEAR — the standard NEP-141 storage deposit.
pub const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(1_250_000_000_000_000_000_000);

/// Deposit storage on `contract_id` for `account_id`.
///
/// Mirrors `script/ci/storage-deposit.sh`.
pub async fn run(
    client: &near_fetch::Client,
    account_id: AccountId,
    secret_key: SecretKey,
    contract_id: AccountId,
) -> anyhow::Result<()> {
    tracing::info!(%account_id, %contract_id, deposit = %STORAGE_DEPOSIT_AMOUNT, "Depositing storage");

    let signer = InMemorySigner::from_secret_key(account_id.clone(), secret_key);

    client
        .call(&signer, &contract_id, "storage_deposit")
        .args_json(json!({ "account_id": account_id }))
        .deposit(STORAGE_DEPOSIT_AMOUNT)
        .max_gas()
        .transact()
        .await
        .with_context(|| format!("storage_deposit on {contract_id}"))?
        .into_result()
        .with_context(|| format!("storage_deposit execution on {contract_id}"))?;

    tracing::info!(%account_id, %contract_id, "Storage deposit complete");
    Ok(())
}
