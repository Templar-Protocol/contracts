use near_contract_standards::fungible_token::metadata::FungibleTokenMetadata;
use near_jsonrpc_client::{methods::query::RpcQueryRequest, JsonRpcClient};
use near_primitives::{
    types::{AccountId, BlockReference},
    views::{AccountView, QueryRequest},
};

use near_jsonrpc_primitives::types::query::QueryResponseKind;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use templar_common::oracle::pyth::PriceIdentifier;

use crate::{CliError, CliResult};

/// View account state from NEAR blockchain
/// # Errors
pub async fn view_account(client: &JsonRpcClient, account_id: AccountId) -> CliResult<AccountView> {
    let account_state = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccount { account_id },
        })
        .await?;

    let QueryResponseKind::ViewAccount(result) = account_state.kind else {
        return Err(CliError::WrongResponseKind(format!(
            "Expected CallResult got {:?}",
            account_state.kind
        )));
    };
    Ok(result)
}

#[allow(clippy::expect_used, reason = "We know the serialization will succeed")]
pub fn serialize_and_encode(data: impl Serialize) -> Vec<u8> {
    near_sdk::serde_json::to_vec(&data).expect("Failed to serialize data")
}

/// Check if a price feed exists on the oracle contract
/// # Errors
pub async fn price_feed_exists(
    client: &JsonRpcClient,
    oracle_contract_id: AccountId,
    price_id: &PriceIdentifier,
) -> CliResult<bool> {
    let args = json!({
        "price_identifier": price_id
    });

    let request = RpcQueryRequest {
        block_reference: BlockReference::latest(),
        request: QueryRequest::CallFunction {
            account_id: oracle_contract_id,
            method_name: "price_feed_exists".to_string(),
            args: serialize_and_encode(args).into(),
        },
    };

    let response = client
        .call(request)
        .await
        .map_err(|e| CliError::NearRpc(format!("Failed to query oracle contract: {e}")))?;

    if let QueryResponseKind::CallResult(result) = response.kind {
        let exists: bool = serde_json::from_slice(&result.result)
            .map_err(|e| CliError::Oracle(format!("Failed to parse response: {e}")))?;
        Ok(exists)
    } else {
        Err(CliError::Oracle(
            "Unexpected response type from oracle".into(),
        ))
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct MultiTokenMetadata {
    icon: String,
    id: String,
    name: String,
    symbol: String,
    decimals: u8,
}

fn parse_multi_token_metadata(data: &[u8]) -> CliResult<MultiTokenMetadata> {
    serde_json::from_slice(data).or_else(|_| {
        let mut list: Vec<MultiTokenMetadata> = serde_json::from_slice(data)?;
        list.pop()
            .ok_or_else(|| CliError::InvalidOutput("Empty metadata list".into()))
    })
}

/// Fetch decimals for a fungible asset (NEP-141 or NEP-245) via RPC
/// # Errors
pub async fn token_metadata(
    client: &JsonRpcClient,
    contract_id: AccountId,
    token_id: Option<String>,
) -> CliResult<u8> {
    let (method_name, args) = if let Some(token_id) = token_id.as_ref() {
        (
            "mt_metadata_base_by_token_id".to_string(),
            json!({ "token_ids": [token_id] }),
        )
    } else {
        ("ft_metadata".to_string(), json!({}))
    };

    let request = RpcQueryRequest {
        block_reference: BlockReference::latest(),
        request: QueryRequest::CallFunction {
            account_id: contract_id.clone(),
            method_name: method_name.clone(),
            args: serialize_and_encode(args).into(),
        },
    };

    let response = client.call(request).await.map_err(|e| {
        CliError::NearRpc(format!(
            "Failed to query {method_name} for {contract_id}: {e}"
        ))
    })?;

    let QueryResponseKind::CallResult(result) = response.kind else {
        return Err(CliError::NearRpc(
            "Unexpected response type from token metadata query".into(),
        ));
    };

    if method_name == "mt_metadata_base_by_token_id" {
        let metadata: MultiTokenMetadata =
            parse_multi_token_metadata(&result.result).map_err(|e| {
                CliError::NearRpc(format!(
                    "Failed to parse mt_metadata_base_by_token_id response: {e}"
                ))
            })?;

        Ok(metadata.decimals)
    } else {
        let metadata: FungibleTokenMetadata = serde_json::from_slice(&result.result)
            .map_err(|e| CliError::NearRpc(format!("Failed to parse ft_metadata response: {e}")))?;

        Ok(metadata.decimals)
    }
}

/// Generic function to call a view method on a NEAR contract via RPC
/// # Errors
pub async fn view<T: DeserializeOwned>(
    client: &JsonRpcClient,
    account_id: AccountId,
    function_name: &str,
    args: impl Serialize,
) -> CliResult<T> {
    let access_key_query_response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::CallFunction {
                account_id,
                method_name: function_name.to_owned(),
                args: serialize_and_encode(&args).into(),
            },
        })
        .await?;

    let QueryResponseKind::CallResult(result) = access_key_query_response.kind else {
        return Err(CliError::WrongResponseKind(format!(
            "Expected CallResult got {:?}",
            access_key_query_response.kind
        )));
    };

    Ok(near_sdk::serde_json::from_slice(&result.result)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_json::json;

    #[test]
    fn serialize_and_encode_matches_json() {
        let payload = json!({ "example": 1, "string": "ok" });
        let encoded = serialize_and_encode(&payload);
        assert_eq!(encoded, serde_json::to_vec(&payload).unwrap());
    }

    #[rstest]
    #[case::single(json!({"icon":"x","id":"a","name":"A","symbol":"A","decimals":6}))]
    #[case::wrapped_list(json!([{"icon":"x","id":"a","name":"A","symbol":"A","decimals":6}]))]
    fn parse_multi_token_metadata_handles_single_and_list(#[case] payload: serde_json::Value) {
        let bytes = serde_json::to_vec(&payload).unwrap();
        let meta = parse_multi_token_metadata(&bytes).expect("metadata should parse");
        assert_eq!(meta.decimals, 6);
        assert_eq!(meta.symbol, "A");
    }
}
