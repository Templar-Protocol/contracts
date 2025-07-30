use anyhow::bail;
use base64::Engine;
use near_crypto::InMemorySigner;
use near_jsonrpc_client::{
    JsonRpcClient,
    methods::{
        query::RpcQueryRequest,
        send_tx::RpcSendTransactionRequest,
        tx::{RpcTransactionError, RpcTransactionStatusRequest, TransactionInfo},
    },
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction},
    types::{AccountId, BlockReference},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    serde::{Serialize, de::DeserializeOwned},
    serde_json,
};
use tokio::time::Instant;
use tracing::instrument;

#[instrument(skip(client), level = "debug")]
pub async fn get_access_key_data(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
) -> anyhow::Result<(u64, CryptoHash)> {
    let access_key_query_response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: signer.account_id.clone(),
                public_key: signer.public_key().clone(),
            },
        })
        .await?;

    let nonce = match access_key_query_response.kind {
        QueryResponseKind::AccessKey(access_key) => access_key.nonce + 1,
        _ => bail!("failed to extract current nonce"),
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
) -> anyhow::Result<T> {
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
        bail!("failed to extract current nonce");
    };

    Ok(serde_json::from_slice(&result.result)?)
}

#[instrument(skip(client, signer), level = "debug")]
pub async fn send_tx(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
    timeout: u64,
    tx: Transaction,
) -> anyhow::Result<FinalExecutionStatus> {
    let (tx_hash, _size) = tx.get_hash_and_size();

    let called_at = Instant::now();
    let signature = signer.sign(tx_hash.as_ref());
    let result = match client
        .call(RpcSendTransactionRequest {
            signed_transaction: SignedTransaction::new(signature, tx),
            wait_until: TxExecutionStatus::Final,
        })
        .await
    {
        Ok(res) => res,
        Err(e) => {
            match e.handler_error() {
                Some(RpcTransactionError::TimeoutError) => {}
                _ => Err(e)?,
            }
            loop {
              if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                  return Err(e.into());
              }

              // Poll with exponential backoff
              let mut poll_interval = Duration::from_millis(500);
              const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

              loop {
                  if Instant::now() >= deadline {
                      bail!("Transaction timeout after {}s", timeout);
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

                  match status {
                      Ok(status) => break status,
                      Err(e) => {
                          if !matches!(e.handler_error(), Some(RpcTransactionError::TimeoutError)) {
                              return Err(e.into());
                          }
                      }
                  }
              }
        }
    };

    let Some(outcome) = result.final_execution_outcome else {
        bail!("Transaction did not return a final execution outcome");
    };

    Ok(outcome.into_outcome().status)
}
