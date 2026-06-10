// SPDX-License-Identifier: MIT
//! RPC utilities for interacting with NEAR blockchain.
//!
//! This module provides helper functions for common NEAR RPC operations:
//! - `view()` - Query view methods on contracts
//! - `send_tx()` - Send signed transactions with retry logic
//! - `get_access_key_data()` - Fetch nonce and block hash for transaction signing
//! - `list_deployments()` - Paginated fetching of market deployments from registries
//!
//! # Error Handling
//!
//! All RPC operations return `AccumulatorResult<T>` which wraps accumulator error variants.

use std::{collections::HashMap, time::Duration};

use futures::{StreamExt, TryStreamExt};
use near_crypto::Signer;
use near_jsonrpc_client::methods::{
    query::RpcQueryRequest,
    send_tx::RpcSendTransactionRequest,
    tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
};
use near_jsonrpc_client::JsonRpcClient;
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction},
    types::{AccountId, BlockReference},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    near,
    serde::{de::DeserializeOwned, Serialize},
    Gas,
};
use templar_common::borrow::BorrowPosition;
use tokio::time::Instant;
use tracing::instrument;

use crate::{AccumulatorError, AccumulatorResult};

/// Borrow positions map type
pub type BorrowPositions = HashMap<AccountId, BorrowPosition>;

/// Default gas for transactions. 300 `TGas`.
pub const DEFAULT_GAS: u64 = Gas::from_tgas(300).as_gas();

/// Maximum interval between transaction status polls
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Network configuration for NEAR
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
#[near(serializers = [near_sdk::serde_json::json])]
pub enum Network {
    /// NEAR mainnet
    Mainnet,
    /// NEAR testnet (default)
    #[default]
    Testnet,
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Network::Mainnet => "mainnet",
                Network::Testnet => "testnet",
            }
        )
    }
}

impl Network {
    /// Get the RPC URL for this network
    #[must_use]
    pub fn rpc_url(&self) -> &str {
        match self {
            Network::Mainnet => "https://rpc.mainnet.fastnear.com",
            Network::Testnet => "https://rpc.testnet.fastnear.com",
        }
    }
}

/// Get access key data (nonce and block hash) for transaction signing.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `signer` - Signer with the account and key to query
///
/// # Returns
///
/// Tuple of (nonce, `block_hash`) to use when constructing a transaction
#[instrument(skip(client), level = "debug")]
pub async fn get_access_key_data(
    client: &JsonRpcClient,
    signer: &Signer,
) -> AccumulatorResult<(u64, CryptoHash)> {
    let access_key_query_response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: signer.get_account_id(),
                public_key: signer.public_key().clone(),
            },
        })
        .await
        .map_err(AccumulatorError::AccessKeyDataError)?;

    let nonce = match access_key_query_response.kind {
        QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
        _ => {
            return Err(AccumulatorError::WrongResponseKind(format!(
                "Expected AccessKey got {:?}",
                access_key_query_response.kind
            )));
        }
    };
    let block_hash = access_key_query_response.block_hash;

    Ok((nonce, block_hash))
}

/// Check if an account ID exists on NEAR.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `account_id` - Account ID to check
///
/// # Returns
///
/// True if the account exists, false otherwise
#[instrument(skip(client), level = "debug")]
pub async fn account_exists(
    client: &JsonRpcClient,
    account_id: &AccountId,
) -> AccumulatorResult<bool> {
    let result = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccount {
                account_id: account_id.clone(),
            },
        })
        .await;

    match result {
        Ok(_) => Ok(true),
        Err(e) => {
            if e.handler_error().is_some() {
                Ok(false)
            } else {
                Err(AccumulatorError::ViewMethodError(e))
            }
        }
    }
}

/// Serialize and encode data for NEAR contract calls.
///
/// # Panics
///
/// Panics if serialization fails (which should never happen for valid types)
#[allow(clippy::expect_used, reason = "We know the serialization will succeed")]
pub fn serialize_and_encode(data: impl Serialize) -> Vec<u8> {
    near_sdk::serde_json::to_vec(&data).expect("Failed to serialize data")
}

/// Call a view method on a NEAR contract.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `account_id` - Contract account to call
/// * `function_name` - Name of the view method
/// * `args` - Arguments to pass (will be JSON serialized)
///
/// # Returns
///
/// Deserialized response of type T
#[instrument(skip_all, level = "debug", fields(account_id = %account_id, method_name = %function_name, args = ?near_sdk::serde_json::to_string(&args)))]
pub async fn view<T: DeserializeOwned>(
    client: &JsonRpcClient,
    account_id: AccountId,
    function_name: &str,
    args: impl Serialize,
) -> AccumulatorResult<T> {
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
        return Err(AccumulatorError::WrongResponseKind(format!(
            "Expected CallResult got {:?}",
            access_key_query_response.kind
        )));
    };

    Ok(near_sdk::serde_json::from_slice(&result.result)?)
}

/// Send a signed transaction to NEAR with retry logic.
///
/// This function handles:
/// - Transaction signing
/// - Timeout handling with exponential backoff
/// - Automatic retry on timeout errors
/// - Transaction status polling
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `signer` - Signer to sign the transaction
/// * `timeout` - Maximum time to wait in seconds
/// * `tx` - Unsigned transaction to send
///
/// # Returns
///
/// Final execution status of the transaction
#[instrument(skip(client, signer), level = "debug")]
pub async fn send_tx(
    client: &JsonRpcClient,
    signer: &Signer,
    timeout: u64,
    tx: Transaction,
) -> AccumulatorResult<FinalExecutionStatus> {
    let (tx_hash, _size) = tx.get_hash_and_size();

    let called_at = Instant::now();
    let signature = signer.sign(tx_hash.as_ref());
    let deadline = called_at + Duration::from_secs(timeout);
    let result = match client
        .call(RpcSendTransactionRequest {
            signed_transaction: SignedTransaction::new(signature, tx),
            wait_until: TxExecutionStatus::Final,
        })
        .await
    {
        Ok(res) => res,
        Err(e) => {
            loop {
                if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                    return Err(e.into());
                }

                // Poll with exponential backoff
                let mut poll_interval = Duration::from_millis(500);

                loop {
                    if Instant::now() >= deadline {
                        return Err(AccumulatorError::TimeoutError(
                            timeout,
                            called_at.elapsed().as_secs(),
                        ));
                    }

                    tokio::time::sleep(poll_interval).await;

                    // Exponential backoff up to MAX_POLL_INTERVAL
                    poll_interval = std::cmp::min(poll_interval * 2, MAX_POLL_INTERVAL);

                    let status = client
                        .call(RpcTransactionStatusRequest {
                            transaction_info: TransactionInfo::TransactionId {
                                sender_account_id: signer.get_account_id(),
                                tx_hash,
                            },
                            wait_until: TxExecutionStatus::Final,
                        })
                        .await;

                    let Err(e) = status else {
                        break;
                    };

                    if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                        return Err(e.into());
                    }
                }
            }
        }
    };

    let Some(outcome) = result.final_execution_outcome else {
        return Err(AccumulatorError::NoOutcome(tx_hash.to_string()));
    };

    Ok(outcome.into_outcome().status)
}

/// List all deployments from a single registry contract.
///
/// Fetches all markets in pages of 500 until no more results.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `registry` - Registry contract account
/// * `_count` - Unused (kept for API compatibility)
/// * `_offset` - Unused (kept for API compatibility)
///
/// # Returns
///
/// Vector of all deployed market accounts
#[instrument(skip(client), level = "debug")]
#[allow(clippy::used_underscore_binding)]
pub async fn list_deployments(
    client: &JsonRpcClient,
    registry: AccountId,
    _count: Option<u32>,
    _offset: Option<u32>,
) -> AccumulatorResult<Vec<AccountId>> {
    let mut all_deployments = Vec::new();
    let page_size = 500;
    let mut current_offset = 0;

    loop {
        let params = near_sdk::serde_json::json!({
            "offset": current_offset,
            "count": page_size,
        });

        let page =
            view::<Vec<AccountId>>(client, registry.clone(), "list_deployments", params).await?;

        let fetched = page.len();

        if fetched == 0 {
            break;
        }

        all_deployments.extend(page);
        current_offset += fetched;

        if fetched < page_size {
            break;
        }
    }

    Ok(all_deployments)
}

/// Contract source metadata as defined by NEP-330
#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct ContractSourceMetadata {
    /// Contract version (semver format)
    pub version: String,
    /// Link to source code repository
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Standards implemented by the contract
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standards: Option<Vec<Standard>>,
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub struct Standard {
    pub standard: String,
    pub version: String,
}

pub fn is_v1_0_0(version: &str) -> bool {
    version == "1.0.0"
}

/// Get contract source metadata (NEP-330)
///
/// Returns `None` if the contract doesn't implement NEP-330 or the call fails.
pub async fn get_contract_version(
    client: &JsonRpcClient,
    contract_id: &AccountId,
) -> Option<String> {
    let result: Result<ContractSourceMetadata, AccumulatorError> = view(
        client,
        contract_id.clone(),
        "contract_source_metadata",
        near_sdk::serde_json::json!({}),
    )
    .await;

    match result {
        Ok(metadata) => Some(metadata.version),
        Err(_) => None,
    }
}

/// List all deployments from multiple registry contracts concurrently.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `registries` - Vector of registry contract accounts
/// * `concurrency` - Maximum number of concurrent requests
///
/// # Returns
///
/// Vector of all deployed market accounts from all registries
#[instrument(skip(client), level = "debug")]
pub async fn list_all_deployments(
    client: JsonRpcClient,
    registries: Vec<AccountId>,
    concurrency: usize,
) -> AccumulatorResult<Vec<AccountId>> {
    let all_markets: Vec<AccountId> = futures::stream::iter(registries)
        .map(|registry| {
            let client = client.clone();
            async move { list_deployments(&client, registry, None, None).await }
        })
        .buffer_unordered(concurrency)
        .try_concat()
        .await?;

    let existing = futures::stream::iter(all_markets.into_iter())
        .filter(|market_id| {
            let client = client.clone();
            let market_id = market_id.clone();
            async move { account_exists(&client, &market_id).await.unwrap_or(false) }
        })
        .collect::<Vec<AccountId>>()
        .await;

    Ok(existing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    use near_jsonrpc_primitives::types::query::RpcQueryResponse;
    use near_primitives::types::AccountId;
    use near_sdk::serde_json::{json, Value as JsonValue};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::body_string_contains;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    fn rpc_success_response(payload: &JsonValue, id: &JsonValue) -> JsonValue {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": payload,
        })
    }

    fn call_result_response(result_bytes: Vec<u8>) -> RpcQueryResponse {
        RpcQueryResponse {
            kind: QueryResponseKind::CallResult(near_primitives::views::CallResult {
                result: result_bytes,
                logs: Vec::new(),
            }),
            block_height: 1,
            block_hash: near_primitives::hash::CryptoHash::default(),
        }
    }

    fn decode_args(params: &JsonValue) -> JsonValue {
        let args_base64 = params
            .get("args_base64")
            .and_then(JsonValue::as_str)
            .expect("args_base64 present");
        let decoded = BASE64_STANDARD
            .decode(args_base64)
            .expect("args_base64 to decode");

        near_sdk::serde_json::from_slice(&decoded).expect("decoded args to be valid json")
    }

    fn parse_query_request(request: &Request) -> (JsonValue, JsonValue) {
        let body: JsonValue =
            near_sdk::serde_json::from_slice(&request.body).expect("request body to be valid json");
        let params = body
            .get("params")
            .cloned()
            .expect("query params to exist in request");
        let id = body.get("id").cloned().unwrap_or_else(|| json!("1"));

        (params, id)
    }

    #[tokio::test]
    async fn list_deployments_paginates_until_short_page() {
        let server = MockServer::start().await;
        let client = JsonRpcClient::connect(server.uri());
        let call_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&call_counter);

        let first_page: Vec<AccountId> = (0..500)
            .map(|idx| format!("market-{idx}.testnet").parse().unwrap())
            .collect();
        let second_page: Vec<AccountId> = vec!["market-500.testnet".parse().unwrap()];

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("list_deployments")
                );

                let offset = decode_args(&params)
                    .get("offset")
                    .and_then(JsonValue::as_u64)
                    .expect("offset present");
                counter_clone.fetch_add(1, Ordering::SeqCst);
                let page = if offset == 0 {
                    &first_page
                } else {
                    &second_page
                };

                let payload = call_result_response(near_sdk::serde_json::to_vec(page).unwrap());
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let deployments =
            list_deployments(&client, "registry.testnet".parse().unwrap(), None, None)
                .await
                .unwrap();

        assert_eq!(deployments.len(), 501);
        assert_eq!(call_counter.load(Ordering::SeqCst), 2);
        assert_eq!(deployments.first().unwrap().as_str(), "market-0.testnet");
        assert_eq!(deployments.last().unwrap().as_str(), "market-500.testnet");
    }

    #[tokio::test]
    async fn list_all_deployments_merges_registries() {
        let server = MockServer::start().await;
        let client = JsonRpcClient::connect(server.uri());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("list_deployments"))
            .respond_with(move |req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("list_deployments")
                );
                calls_clone.fetch_add(1, Ordering::SeqCst);

                let registry_id = params
                    .get("account_id")
                    .and_then(JsonValue::as_str)
                    .expect("registry id");
                let markets: Vec<AccountId> = match registry_id {
                    "registry-a.testnet" => vec!["ma.testnet".parse().unwrap()],
                    "registry-b.testnet" => vec![
                        "mb1.testnet".parse().unwrap(),
                        "mb2.testnet".parse().unwrap(),
                    ],
                    other => panic!("unexpected registry {other}"),
                };
                let payload = call_result_response(near_sdk::serde_json::to_vec(&markets).unwrap());
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("view_account"))
            .respond_with(move |req: &Request| {
                let (_params, id) = parse_query_request(req);
                let response = json!({
                    "amount": "4686230356236922693424338633",
                    "block_hash": "5dFRkorSHHyeMc77auarw2jJ67CAnBiExh3bbhNStfC9",
                    "block_height": 175_548_555,
                    "code_hash": "11111111111111111111111111111111",
                    "locked": "0",
                    "storage_paid_at": 0,
                    "storage_usage": 28677,
                });
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&response, &id))
            })
            .mount(&server)
            .await;

        let deployments = list_all_deployments(
            client,
            vec![
                "registry-a.testnet".parse().unwrap(),
                "registry-b.testnet".parse().unwrap(),
            ],
            2,
        )
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            deployments,
            vec![
                AccountId::from_str("ma.testnet").unwrap(),
                AccountId::from_str("mb1.testnet").unwrap(),
                AccountId::from_str("mb2.testnet").unwrap()
            ]
        );
    }

    #[tokio::test]
    async fn get_contract_version_returns_version() {
        let server = MockServer::start().await;
        let client = JsonRpcClient::connect(server.uri());

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(|req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("contract_source_metadata")
                );

                let metadata = ContractSourceMetadata {
                    version: "2.1.3".to_string(),
                    link: None,
                    standards: None,
                };
                let payload =
                    call_result_response(near_sdk::serde_json::to_vec(&metadata).unwrap());
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let version =
            get_contract_version(&client, &AccountId::from_str("market.testnet").unwrap()).await;

        assert_eq!(version.as_deref(), Some("2.1.3"));
    }

    #[tokio::test]
    async fn get_contract_version_returns_none_on_error() {
        let server = MockServer::start().await;
        let client = JsonRpcClient::connect(server.uri());

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(|req: &Request| {
                let (params, id) = parse_query_request(req);
                assert_eq!(
                    params.get("method_name").and_then(JsonValue::as_str),
                    Some("contract_source_metadata")
                );

                // Return invalid JSON payload to force a deserialize error
                let payload = call_result_response(vec![0_u8]);
                ResponseTemplate::new(200).set_body_json(rpc_success_response(&json!(payload), &id))
            })
            .mount(&server)
            .await;

        let version =
            get_contract_version(&client, &AccountId::from_str("market.testnet").unwrap()).await;

        assert!(version.is_none());
    }
}
