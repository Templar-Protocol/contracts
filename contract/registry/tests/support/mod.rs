use anyhow::{Context, Result};
use near_api::types::AccountId;
use templar_gateway_types::{common::WriteOperationResult, operation::ReceiptStatus};

/// The `templar-registry` events named `event` in the registry's successful receipts during the
/// operation's final transaction, retaining the envelope for version-sensitive assertions.
pub fn registry_event_logs(
    result: &WriteOperationResult,
    registry_id: &AccountId,
    event: &str,
) -> Result<Vec<serde_json::Value>> {
    let logs = result
        .operation
        .final_outcome()
        .context("the operation has an outcome")?
        .receipts
        .iter()
        .filter(|receipt| {
            &receipt.contract_id == registry_id && receipt.status == ReceiptStatus::Succeeded
        })
        .flat_map(|receipt| &receipt.logs)
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(logs
        .into_iter()
        .filter(|log| log["standard"] == "templar-registry" && log["event"] == event)
        .collect())
}
