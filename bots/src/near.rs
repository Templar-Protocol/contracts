use std::collections::HashMap;

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
    action::{Action, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{AccountId, BlockReference},
    views::{FinalExecutionStatus, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    serde::{Serialize, de::DeserializeOwned},
    serde_json::{self, json},
};
use templar_common::{
    borrow::{BorrowPosition, BorrowStatus},
    oracle::pyth::OracleResponse,
};
use tokio::time::Instant;
use tracing::instrument;

use crate::{DEFAULT_GAS, ONE_NEAR};

#[instrument(skip(client), level = "debug")]
async fn get_access_key_data(
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

fn encode(data: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(data.as_bytes())
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
                args: encode(&serde_json::to_string(&args)?).into_bytes().into(),
            },
        })
        .await?;

    match access_key_query_response.kind {
        QueryResponseKind::CallResult(result) => Ok(serde_json::from_slice(&result.result)?),
        _ => bail!("failed to extract current nonce"),
    }
}

#[instrument(skip(client), level = "debug")]
pub async fn get_borrow_status(
    client: &JsonRpcClient,
    market: AccountId,
    borrow: AccountId,
    oracle_response: &OracleResponse,
) -> anyhow::Result<Option<BorrowStatus>> {
    let status_res = view(
        client,
        market,
        "get_borrow_status",
        &json!({
            "account_id": borrow,
            "oracle_response": oracle_response,
        }),
    )
    .await?;
    Ok(status_res)
}

#[instrument(skip(client), level = "debug")]
pub async fn get_borrow_position(
    client: &JsonRpcClient,
    market: AccountId,
    borrow: AccountId,
) -> anyhow::Result<BorrowPosition> {
    let status_res = view(
        client,
        market,
        "get_borrow_position",
        &json!({
            "account_id": borrow,
        }),
    )
    .await?;
    Ok(status_res)
}

#[instrument(skip(client), level = "debug")]
pub async fn get_borrows(
    client: &JsonRpcClient,
    market: &AccountId,
    offset: Option<u32>,
    count: Option<u32>,
) -> anyhow::Result<HashMap<AccountId, BorrowPosition>> {
    type BorrowPositions = HashMap<AccountId, BorrowPosition>;
    let mut all_positions: BorrowPositions = HashMap::new();

    let page_size = 100;
    let mut current_offset = 0;
    let mut params = json!({
        "offset": current_offset,
        "count": page_size,
    });

    while let Ok(page) = view::<BorrowPositions>(
        client,
        market.clone(),
        "list_borrow_positions",
        params.clone(),
    )
    .await
    {
        let fetched = page.len();
        all_positions.extend(page);
        current_offset += page_size;
        params["offset"] = current_offset.into();

        if fetched < page_size {
            break;
        }
    }

    Ok(all_positions)
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
    let signature = signer.sign(tx.get_hash_and_size().0.as_ref());
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
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                let status = client
                    .call(RpcTransactionStatusRequest {
                        transaction_info: TransactionInfo::TransactionId {
                            sender_account_id: signer.account_id.clone(),
                            tx_hash,
                        },
                        wait_until: TxExecutionStatus::Final,
                    })
                    .await;
                let elapsed = called_at.elapsed().as_secs();

                if elapsed > timeout {
                    bail!("Transaction timeout");
                }

                match status {
                    Ok(status) => break status,
                    Err(e) => match e.handler_error() {
                        Some(RpcTransactionError::TimeoutError) => {}
                        _ => Err(e)?,
                    },
                }
            }
        }
    };

    let Some(outcome) = result.final_execution_outcome else {
        bail!("Transaction did not return a final execution outcome");
    };

    Ok(outcome.into_outcome().status)
}

#[instrument(skip_all, level = "debug", fields(
    account_id = %signer.account_id,
    ft_contract = %ft_contract,
    args = ?serde_json::to_string(&args)
))]
pub async fn ft_transfer_call(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
    ft_contract: AccountId,
    args: impl Serialize,
    timeout: u64,
) -> anyhow::Result<FinalExecutionStatus> {
    let (nonce, block_hash) = get_access_key_data(client, signer).await?;

    let tx = Transaction::V0(TransactionV0 {
        nonce,
        receiver_id: ft_contract,
        block_hash,
        signer_id: signer.account_id.clone(),
        public_key: signer.public_key().clone(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer_call".to_string(),
            args: encode(&serde_json::to_string(&args)?).into_bytes(),
            gas: DEFAULT_GAS,
            deposit: ONE_NEAR,
        }))],
    });

    send_tx(client, signer, timeout, tx).await
}

#[instrument(skip_all, level = "debug", fields(
    account_id = %signer.account_id,
    ft_contract = %ft_contract,
    args = ?serde_json::to_string(&args)
))]
pub async fn call_apply_interest(
    client: &JsonRpcClient,
    signer: &InMemorySigner,
    ft_contract: AccountId,
    args: impl Serialize,
    timeout: u64,
) -> anyhow::Result<FinalExecutionStatus> {
    let (nonce, block_hash) = get_access_key_data(client, signer).await?;

    let tx = Transaction::V0(TransactionV0 {
        nonce,
        receiver_id: ft_contract,
        block_hash,
        signer_id: signer.account_id.clone(),
        public_key: signer.public_key().clone(),
        actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "apply_interest".to_string(),
            args: encode(&serde_json::to_string(&args)?).into_bytes(),
            gas: DEFAULT_GAS,
            deposit: 0,
        }))],
    });

    send_tx(client, signer, timeout, tx).await
}
