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

use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};

use futures::{StreamExt, TryStreamExt};
use near_crypto::Signer;
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{
        query::{RpcQueryError, RpcQueryRequest},
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction},
    types::{AccountId, BlockReference},
    views::{FinalExecutionOutcomeView, FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    serde::{de::DeserializeOwned, Deserialize, Serialize},
    Gas,
};
use templar_common::borrow::BorrowPosition;
use tokio::time::Instant;

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

/// Shared nonce tracker to prevent nonce collisions between concurrent transactions
/// (e.g., oracle price updates and liquidation transactions).
///
/// Tracks the last nonce used by any transaction. When fetching a new nonce,
/// returns `max(rpc_nonce, tracked_nonce) + 1` to avoid stale-nonce races
/// where an RPC node hasn't indexed the latest transaction yet.
#[derive(Debug, Clone, Default)]
pub struct NonceTracker(Arc<AtomicU64>);

impl NonceTracker {
    /// Record a nonce that was just used in a transaction.
    pub fn record_used(&self, nonce: u64) {
        self.0.fetch_max(nonce, std::sync::atomic::Ordering::SeqCst);
    }

    /// Given an RPC-reported access key nonce, return the next safe nonce to use.
    ///
    /// Atomically reserves a unique nonce via CAS loop so concurrent callers
    /// never receive the same value.
    pub fn next_nonce(&self, rpc_access_key_nonce: u64) -> u64 {
        let mut observed = self.0.load(std::sync::atomic::Ordering::SeqCst);
        loop {
            let next = rpc_access_key_nonce.max(observed) + 1;
            match self.0.compare_exchange(
                observed,
                next,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            ) {
                Ok(_) => return next,
                Err(current) => observed = current,
            }
        }
    }
}

/// Shared nonce tracker handle.
pub type SharedNonceTracker = NonceTracker;

/// Default timeout for view call requests (seconds)
const VIEW_CALL_TIMEOUT_SECS: u64 = 30;

/// Maximum interval between transaction status polls
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Contract source metadata as defined by NEP-330
#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Standard {
    pub standard: String,
    pub version: String,
}

/// Get contract source metadata (NEP-330)
///
/// Returns `None` if the contract doesn't implement NEP-330 or the call fails.
pub async fn get_contract_version(
    client: &JsonRpcClient,
    contract_id: &AccountId,
) -> Option<String> {
    let result: Result<ContractSourceMetadata, RpcError> = view(
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

/// Get access key data (nonce and block hash) for transaction signing.
///
/// When a `NonceTracker` is provided, the returned nonce is guaranteed to be
/// higher than any previously used nonce, even if the RPC node hasn't indexed
/// recent transactions yet.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `signer` - Signer with the account and key to query
/// * `nonce_tracker` - Optional shared nonce tracker for collision prevention
///
/// # Returns
///
/// Tuple of (nonce, block_hash) to use when constructing a transaction
#[tracing::instrument(skip(client, nonce_tracker), level = "debug")]
pub async fn get_access_key_data(
    client: &JsonRpcClient,
    signer: &Signer,
    nonce_tracker: Option<&NonceTracker>,
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

    let rpc_nonce = match access_key_query_response.kind {
        QueryResponseKind::AccessKey(access_key) => access_key.nonce,
        _ => {
            return Err(RpcError::WrongResponseKind(format!(
                "Expected AccessKey got {:?}",
                access_key_query_response.kind
            )));
        }
    };
    let block_hash = access_key_query_response.block_hash;

    let nonce = if let Some(tracker) = nonce_tracker {
        tracker.next_nonce(rpc_nonce)
    } else {
        rpc_nonce + 1
    };

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
#[tracing::instrument(skip_all, level = "debug", fields(account_id = %account_id, method_name = %function_name, args = ?near_sdk::serde_json::to_string(&args)))]
pub async fn view<T: DeserializeOwned>(
    client: &JsonRpcClient,
    account_id: AccountId,
    function_name: &str,
    args: impl Serialize,
) -> RpcResult<T> {
    // Add timeout for view calls to prevent hanging
    let timeout_duration = tokio::time::Duration::from_secs(VIEW_CALL_TIMEOUT_SECS);

    let response = tokio::time::timeout(
        timeout_duration,
        client.call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::CallFunction {
                account_id: account_id.clone(),
                method_name: function_name.to_owned(),
                args: serialize_and_encode(&args).into(),
            },
        }),
    )
    .await
    .map_err(|_| RpcError::TimeoutError(VIEW_CALL_TIMEOUT_SECS, VIEW_CALL_TIMEOUT_SECS))??;

    let QueryResponseKind::CallResult(result) = response.kind else {
        return Err(RpcError::WrongResponseKind(format!(
            "Expected CallResult got {:?}",
            response.kind
        )));
    };

    Ok(near_sdk::serde_json::from_slice(&result.result)?)
}

/// Send a signed transaction and wait for finality.
///
/// Returns the full execution outcome including all receipts.
/// Use `check_transaction_success()` to verify if all receipts succeeded.
///
/// # Arguments
///
/// * `client` - JSON-RPC client instance
/// * `signer` - Transaction signer
/// * `timeout` - Maximum seconds to wait for finality
/// * `tx` - Unsigned transaction to send
///
/// # Returns
///
/// Returns `FinalExecutionOutcomeView` containing transaction status and all receipt outcomes
#[tracing::instrument(skip(client, signer), level = "debug")]
pub async fn send_tx(
    client: &JsonRpcClient,
    signer: &Signer,
    timeout: u64,
    tx: Transaction,
) -> RpcResult<FinalExecutionOutcomeView> {
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
            // Check if the error is a timeout that we should retry
            if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                return Err(e.into());
            }

            // Poll with exponential backoff until we get a final result
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

                match status {
                    Ok(result) => break result,
                    Err(e)
                        if matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) =>
                    {
                        // Continue polling on timeout
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    };

    let Some(outcome) = result.final_execution_outcome else {
        return Err(RpcError::NoOutcome(tx_hash.to_string()));
    };

    Ok(outcome.into_outcome())
}

/// Checks if a transaction and all its receipts succeeded.
///
/// A transaction can have status Success but contain failed receipts.
/// This function checks both the transaction status and all receipt outcomes.
///
/// # Arguments
///
/// * `outcome` - The final execution outcome from `send_tx`
///
/// # Returns
///
/// * `Ok(())` if transaction and all receipts succeeded
/// * `Err(String)` with error description if any receipt failed
///
/// # Errors
///
/// Returns an error if the transaction or any receipt failed
pub fn check_transaction_success(outcome: &FinalExecutionOutcomeView) -> Result<(), String> {
    use near_primitives::views::ExecutionStatusView;

    // Check main transaction status
    match &outcome.status {
        FinalExecutionStatus::Failure(err) => {
            return Err(format!("Transaction failed: {err:?}"));
        }
        FinalExecutionStatus::NotStarted => {
            return Err("Transaction not started".to_string());
        }
        FinalExecutionStatus::Started => {
            return Err("Transaction still in progress".to_string());
        }
        FinalExecutionStatus::SuccessValue(_) => {
            // Continue to check receipts
        }
    }

    // Check all receipt outcomes
    for receipt in &outcome.receipts_outcome {
        match &receipt.outcome.status {
            ExecutionStatusView::Failure(err) => {
                // Try to extract the actual error message from TxExecutionError
                let error_msg = extract_error_message(err);
                return Err(format!("Receipt {} failed: {}", receipt.id, error_msg));
            }
            ExecutionStatusView::Unknown => {
                return Err(format!("Receipt {} status unknown", receipt.id));
            }
            ExecutionStatusView::SuccessValue(_) | ExecutionStatusView::SuccessReceiptId(_) => {
                // This receipt succeeded, continue checking others
            }
        }
    }

    Ok(())
}

/// Extracts a human-readable error message from `TxExecutionError`
fn extract_error_message(err: &near_primitives::errors::TxExecutionError) -> String {
    use near_primitives::errors::{ActionErrorKind, TxExecutionError};

    match err {
        TxExecutionError::ActionError(action_err) => {
            match &action_err.kind {
                ActionErrorKind::FunctionCallError(fc_err) => {
                    // Extract the actual contract panic message
                    format!("{fc_err:?}")
                }
                other => format!("{other:?}"),
            }
        }
        TxExecutionError::InvalidTxError(_) => format!("{err:?}"),
    }
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
#[tracing::instrument(skip(client), level = "debug")]
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
#[tracing::instrument(skip(client), level = "debug")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use near_jsonrpc_client::{NEAR_MAINNET_RPC_URL, NEAR_TESTNET_RPC_URL};
    use near_sdk::serde_json::json;
    use templar_common::utils::Network;

    #[test]
    fn test_serialize_and_encode() {
        let data = json!({"key": "value", "number": 42});
        let encoded = serialize_and_encode(&data);

        // Should be valid JSON bytes
        assert!(!encoded.is_empty());

        // Should be able to deserialize back
        let decoded: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded["key"], "value");
        assert_eq!(decoded["number"], 42);
    }

    #[test]
    fn test_serialize_and_encode_empty_object() {
        let data = json!({});
        let encoded = serialize_and_encode(&data);
        assert_eq!(encoded, b"{}");
    }

    #[test]
    fn test_serialize_and_encode_array() {
        let data = json!([1, 2, 3]);
        let encoded = serialize_and_encode(&data);
        let decoded: Vec<u32> = near_sdk::serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, vec![1, 2, 3]);
    }

    #[test]
    fn test_network_display() {
        assert_eq!(Network::Mainnet.to_string(), "mainnet");
        assert_eq!(Network::Testnet.to_string(), "testnet");
    }

    #[test]
    fn test_network_rpc_url() {
        assert_eq!(Network::Mainnet.rpc_url(), NEAR_MAINNET_RPC_URL);
        assert_eq!(Network::Testnet.rpc_url(), NEAR_TESTNET_RPC_URL);
    }

    #[test]
    fn test_network_default() {
        let network = Network::default();
        assert_eq!(network.to_string(), "testnet");
    }

    #[test]
    fn test_rpc_error_display() {
        let error = RpcError::WrongResponseKind("unexpected type".to_string());
        let display = format!("{error}");
        assert!(display.contains("unexpected type"));
    }

    #[test]
    fn test_app_error_from_rpc_error() {
        let rpc_error = RpcError::WrongResponseKind("test".to_string());
        let app_error: AppError = rpc_error.into();
        let display = format!("{app_error}");
        assert!(display.contains("RPC error"));
    }

    #[test]
    fn test_timeout_error_display() {
        let error = RpcError::TimeoutError(60, 65);
        let display = format!("{error}");
        assert!(display.contains("60"));
        assert!(display.contains("65"));
    }

    #[test]
    fn test_nonce_tracker_next_nonce_basic() {
        let tracker = NonceTracker::default();
        // First call with rpc_nonce=10 → 11
        assert_eq!(tracker.next_nonce(10), 11);
        // Second call with same rpc_nonce → 12 (tracked is now 11)
        assert_eq!(tracker.next_nonce(10), 12);
    }

    #[test]
    fn test_nonce_tracker_rpc_jumps_forward() {
        let tracker = NonceTracker::default();
        assert_eq!(tracker.next_nonce(10), 11);
        // RPC reports higher nonce (another source used nonces)
        assert_eq!(tracker.next_nonce(20), 21);
        // Tracked is now 21
        assert_eq!(tracker.next_nonce(15), 22);
    }

    #[test]
    fn test_nonce_tracker_record_used() {
        let tracker = NonceTracker::default();
        // Not a cryptographic nonce — this is a NEAR tx sequence counter.
        let previously_used: u64 = 50; // lgtm[rust/hardcoded-credentials]
        tracker.record_used(previously_used);
        // next_nonce should be above recorded value
        assert_eq!(tracker.next_nonce(10), previously_used + 1);
    }

    #[test]
    fn test_nonce_tracker_concurrent_unique() {
        use std::collections::HashSet;
        use std::sync::Arc;

        let tracker = Arc::new(NonceTracker::default());
        let mut handles = Vec::new();

        for _ in 0..100 {
            let t = tracker.clone();
            handles.push(std::thread::spawn(move || t.next_nonce(10)));
        }

        let nonces: HashSet<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // All 100 nonces must be unique
        assert_eq!(nonces.len(), 100);
        // All should be > 10
        assert!(nonces.iter().all(|&n| n > 10));
    }
}
