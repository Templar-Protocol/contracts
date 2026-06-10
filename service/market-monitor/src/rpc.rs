//! NEAR RPC utilities.

use crate::error::{MonitorError, Result};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use near_sdk::AccountId;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Calls a view function on a NEAR contract.
///
/// # Errors
/// Returns an error if the RPC call fails or response cannot be deserialized.
pub async fn view<T: DeserializeOwned>(
    client: &JsonRpcClient,
    account_id: AccountId,
    method_name: &str,
    args: Value,
) -> Result<T> {
    let args_json = serde_json::to_string(&args)
        .map_err(|e| MonitorError::Rpc(format!("Failed to serialize args: {e}")))?;

    let request = methods::query::RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::CallFunction {
            account_id: account_id.clone(),
            method_name: method_name.to_string(),
            args: FunctionArgs::from(args_json.into_bytes()),
        },
    };

    let response = client
        .call(request)
        .await
        .map_err(|e| MonitorError::Rpc(format!("RPC call failed: {e}")))?;

    match response.kind {
        QueryResponseKind::CallResult(result) => serde_json::from_slice(&result.result)
            .map_err(|e| MonitorError::Rpc(format!("Failed to deserialize response: {e}"))),
        _ => Err(MonitorError::Rpc("Unexpected response type".to_string())),
    }
}

pub async fn get_contract_version(
    client: &JsonRpcClient,
    contract_id: &AccountId,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ContractMetadata {
        version: Option<String>,
    }

    let metadata: Option<ContractMetadata> = view(
        client,
        contract_id.clone(),
        "contract_metadata",
        Value::Null,
    )
    .await
    .ok()?;

    metadata.and_then(|m| m.version)
}
