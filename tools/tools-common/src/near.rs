use std::str::FromStr;

use anyhow::Context;
use near_contract_standards::contract_metadata::ContractSourceMetadata;
use near_crypto::Signer;
use near_sdk::serde_json::Value;
use near_sdk::{AccountId, AccountIdRef};

use crate::version::{RegistryVersion, Version};

/// Return `true` when the account exists on-chain.
///
/// Only returns `Err` for unexpected RPC failures; a missing account yields `Ok(false)`.
pub async fn account_exists(
    near: &near_fetch::Client,
    account_id: &AccountId,
) -> anyhow::Result<bool> {
    match near.view_account(account_id).await {
        Ok(_) => Ok(true),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("does not exist") || msg.contains("UNKNOWN_ACCOUNT") {
                Ok(false)
            } else {
                Err(anyhow::anyhow!(
                    "RPC error checking account {account_id}: {e}"
                ))
            }
        }
    }
}

pub async fn contract_version<T>(
    near: &near_fetch::Client,
    account_id: &AccountId,
) -> anyhow::Result<Version<T>> {
    let contract_metadata: ContractSourceMetadata = near
        .view(account_id, "contract_source_metadata")
        .args(b"{}".to_vec())
        .await?
        .json()?;

    let Some(version_str) = contract_metadata.version else {
        anyhow::bail!("contract_source_metadata does not contain version");
    };

    Ok(Version::<T>::from_str(&version_str)?)
}

/// Call a view method and return the raw JSON value.
pub async fn view(
    near: &near_fetch::Client,
    account_id: &AccountId,
    method: &str,
    args: impl near_sdk::serde::Serialize,
) -> anyhow::Result<Value> {
    let result = near
        .view(account_id, method)
        .args_json(args)
        .await
        .with_context(|| format!("view {method} on {account_id}"))?;
    result
        .json::<Value>()
        .with_context(|| format!("deserialise response from {method}"))
}

/// Delete `account_id`, sending its NEAR balance to `beneficiary_id`.
pub async fn delete_account(
    near: &near_fetch::Client,
    signer: &Signer,
    account_id: &AccountId,
    beneficiary_id: &AccountId,
) -> anyhow::Result<()> {
    near.batch(signer, account_id)
        .delete_account(beneficiary_id)
        .transact()
        .await
        .with_context(|| format!("delete account {account_id}"))?;
    tracing::info!(%account_id, %beneficiary_id, "account deleted");
    Ok(())
}
