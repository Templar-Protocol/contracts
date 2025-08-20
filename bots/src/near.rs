use std::time::Duration;

use base64::Engine;
use near_crypto::InMemorySigner;
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
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json, Gas,
};
use tokio::time::Instant;
use tracing::instrument;

pub const GAS_FT_TRANSFER: Gas = Gas::from_tgas(6);

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
    DeserializeError(#[from] serde_json::Error),
    /// Timeout exceeded
    #[error("Timeout exceeded: {0}")]
    TimeoutError(u64, u64),
    /// No outcome for transaction
    #[error("No outcome for transaction: {0}")]
    NoOutcome(String),
}

pub type RpcResult<T = ()> = Result<T, RpcError>;

#[instrument(skip(client), level = "debug")]
pub async fn get_access_key_data(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
) -> RpcResult<(u64, CryptoHash)> {
    let access_key_query_response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: signer.account_id.clone(),
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

#[allow(clippy::expect_used, reason = "We know the serialization will succeed")]
pub fn serialize_and_encode(data: impl Serialize) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_string(&data).expect("Failed to serialize data"))
        .into_bytes()
}

#[instrument(skip_all, level = "debug", fields(account_id = %account_id, method_name = %function_name, args = ?serde_json::to_string(&args)))]
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

    Ok(serde_json::from_slice(&result.result)?)
}

const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

#[instrument(skip(client, signer), level = "debug")]
pub async fn send_tx(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
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

                    // Exponential backoff
                    poll_interval = std::cmp::min(poll_interval * 2, MAX_POLL_INTERVAL);

                    let status = client
                        .call(RpcTransactionStatusRequest {
                            transaction_info: TransactionInfo::TransactionId {
                                sender_account_id: signer.account_id.clone(),
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
