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
//! All RPC operations return `RpcResult<T>` which wraps various RPC-level errors.
//! These are converted to `LiquidatorError` at the application level.

use std::{collections::HashMap, time::Duration};

use futures::{StreamExt, TryStreamExt};
use near_crypto::Signer;
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{
        query::{RpcQueryError, RpcQueryRequest},
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient, NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL,
};
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

/// Error types for RPC operations
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// Failed to query view method
    #[error("Failed to query view method: {0}")]
    ViewMethodError(#[from] JsonRpcError<RpcQueryError>),
    /// Failed to get access key data
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(JsonRpcError<RpcQueryError>),
    /// Got wrong response kind from RPC
    #[error("Got wrong response kind from RPC: {0}")]
    WrongResponseKind(String),
    /// Failed to send transaction
    #[error("Failed to send transaction: {0}")]
    SendTransactionError(#[from] JsonRpcError<RpcTransactionError>),
    /// Failed to deserialize response
    #[error("Failed to deserialize response: {0}")]
    DeserializeError(#[from] near_sdk::serde_json::Error),
    /// Timeout exceeded
    #[error("Timeout exceeded after {0}s (waited {1}s)")]
    TimeoutError(u64, u64),
    /// No outcome for transaction
    #[error("No outcome for transaction: {0}")]
    NoOutcome(String),
}

/// Error types for application-level operations
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// RPC operation failed
    #[error("RPC error: {0}")]
    Rpc(#[from] RpcError),
    /// Validation error
    #[error("Validation error: {0}")]
    ValidationError(String),
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

pub type RpcResult<T = ()> = Result<T, RpcError>;
pub type AppResult<T = ()> = Result<T, AppError>;

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
            Network::Mainnet => NEAR_MAINNET_RPC_URL,
            Network::Testnet => NEAR_TESTNET_RPC_URL,
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
/// Tuple of (nonce, block_hash) to use when constructing a transaction
#[instrument(skip(client), level = "debug")]
pub async fn get_access_key_data(
    client: &JsonRpcClient,
    signer: &Signer,
) -> RpcResult<(u64, CryptoHash)> {
    let access_key_query_response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: signer.get_account_id(),
                public_key: signer.public_key().clone(),
            },
        })
        .await
        .map_err(RpcError::AccessKeyDataError)?;

    let nonce = match access_key_query_response.kind {
        QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
        _ => {
            return Err(RpcError::WrongResponseKind(format!(
                "Expected AccessKey got {:?}",
                access_key_query_response.kind
            )));
        }
    };
    let block_hash = access_key_query_response.block_hash;

    Ok((nonce, block_hash))
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
) -> RpcResult<T> {
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
        return Err(RpcError::WrongResponseKind(format!(
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
) -> RpcResult<FinalExecutionStatus> {
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
                        return Err(RpcError::TimeoutError(
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
        return Err(RpcError::NoOutcome(tx_hash.to_string()));
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
) -> RpcResult<Vec<AccountId>> {
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
) -> RpcResult<Vec<AccountId>> {
    let all_markets: Vec<AccountId> = futures::stream::iter(registries)
        .map(|registry| {
            let client = client.clone();
            async move { list_deployments(&client, registry, None, None).await }
        })
        .buffer_unordered(concurrency)
        .try_concat()
        .await?;

    Ok(all_markets)
}
