use std::{str::FromStr, time::Duration};

use anyhow::{bail, Context};
use near_contract_standards::contract_metadata::ContractSourceMetadata;
use near_crypto::Signer;
use near_jsonrpc_client::{
    methods::{broadcast_tx_commit::RpcBroadcastTxCommitRequest, query::RpcQueryRequest},
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryError, RpcQueryResponse};
use near_primitives::{
    transaction::{
        Action, DeleteAccountAction, DeployContractAction, FunctionCallAction, SignedTransaction,
        Transaction, TransactionV0,
    },
    types::BlockReference,
    views::{FinalExecutionOutcomeView, FinalExecutionStatus, QueryRequest},
};
use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json::{self},
    AccountId, Gas, NearToken,
};
use templar_gateway_types::Version;
use tokio::time::sleep;

pub type Client = JsonRpcClient;

const MAX_GAS: Gas = Gas::from_tgas(300);
const ACCESS_KEY_LOOKUP_RETRIES: u32 = 5;
const ACCESS_KEY_LOOKUP_DELAY_MS: u64 = 50;

#[derive(Clone, Debug)]
pub struct Function {
    method_name: String,
    args: Vec<u8>,
    gas: Gas,
    deposit: NearToken,
}

impl Function {
    #[must_use]
    pub fn new(method_name: impl Into<String>) -> Self {
        Self {
            method_name: method_name.into(),
            args: b"{}".to_vec(),
            gas: MAX_GAS,
            deposit: NearToken::from_yoctonear(0),
        }
    }

    #[must_use]
    pub fn args(mut self, args: Vec<u8>) -> Self {
        self.args = args;
        self
    }

    pub fn args_json(mut self, args: impl Serialize) -> anyhow::Result<Self> {
        self.args = serde_json::to_vec(&args)?;
        Ok(self)
    }

    #[must_use]
    pub fn deposit(mut self, deposit: NearToken) -> Self {
        self.deposit = deposit;
        self
    }

    #[must_use]
    pub fn gas(mut self, gas: Gas) -> Self {
        self.gas = gas;
        self
    }

    #[must_use]
    pub fn max_gas(self) -> Self {
        self.gas(MAX_GAS)
    }
}

impl From<Function> for Action {
    fn from(value: Function) -> Self {
        Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: value.method_name,
            args: value.args,
            gas: near_primitives::gas::Gas::from_gas(value.gas.as_gas()),
            deposit: value.deposit,
        }))
    }
}

/// Return `true` when the account exists on-chain.
///
/// Only returns `Err` for unexpected RPC failures; a missing account yields `Ok(false)`.
pub async fn account_exists(near: &Client, account_id: &AccountId) -> anyhow::Result<bool> {
    let request = RpcQueryRequest {
        block_reference: BlockReference::latest(),
        request: QueryRequest::ViewAccount {
            account_id: account_id.clone(),
        },
    };

    match near.call(request).await {
        Ok(_) => Ok(true),
        Err(error) => match error.handler_error() {
            Some(RpcQueryError::UnknownAccount { .. }) => Ok(false),
            _ => Err(anyhow::anyhow!(
                "RPC error checking account {account_id}: {error}"
            )),
        },
    }
}

pub async fn contract_version<T>(
    near: &Client,
    account_id: &AccountId,
) -> anyhow::Result<Version<T>> {
    let contract_metadata: ContractSourceMetadata = view(
        near,
        account_id,
        "contract_source_metadata",
        serde_json::json!({}),
    )
    .await?;

    let Some(version_str) = contract_metadata.version else {
        anyhow::bail!("contract_source_metadata does not contain version");
    };

    Ok(Version::<T>::from_str(&version_str)?)
}

/// Call a view method and deserialize the response.
pub async fn view<T: DeserializeOwned>(
    near: &Client,
    account_id: &AccountId,
    method: &str,
    args: impl Serialize,
) -> anyhow::Result<T> {
    let args = serde_json::to_vec(&args)?;
    let result = near
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::CallFunction {
                account_id: account_id.clone(),
                method_name: method.to_owned(),
                args: args.into(),
            },
        })
        .await
        .with_context(|| format!("view {method} on {account_id}"))?;

    let QueryResponseKind::CallResult(result) = result.kind else {
        bail!("Expected CallResult from {method}, got {:?}", result.kind);
    };

    serde_json::from_slice::<T>(&result.result)
        .with_context(|| format!("deserialise response from {method}"))
}

pub async fn send_tx(
    near: &Client,
    signer: &Signer,
    receiver_id: &AccountId,
    actions: Vec<Action>,
) -> anyhow::Result<FinalExecutionOutcomeView> {
    let access_key_response = view_access_key_with_retry(near, signer).await?;

    let QueryResponseKind::AccessKey(access_key) = access_key_response.kind else {
        bail!(
            "Expected AccessKey response, got {:?}",
            access_key_response.kind
        );
    };

    let tx = Transaction::V0(TransactionV0 {
        signer_id: signer.get_account_id(),
        public_key: signer.public_key(),
        nonce: access_key.nonce + 1,
        receiver_id: receiver_id.clone(),
        block_hash: access_key_response.block_hash,
        actions,
    });

    let (tx_hash, _size) = tx.get_hash_and_size();
    let signature = signer.sign(tx_hash.as_ref());
    let signed_transaction = SignedTransaction::new(signature, tx);

    near.call(RpcBroadcastTxCommitRequest { signed_transaction })
        .await
        .context("broadcast_tx_commit")
}

pub async fn send_tx_checked(
    near: &Client,
    signer: &Signer,
    receiver_id: &AccountId,
    actions: Vec<Action>,
) -> anyhow::Result<FinalExecutionOutcomeView> {
    let outcome = send_tx(near, signer, receiver_id, actions).await?;
    require_success_status(&outcome)?;
    Ok(outcome)
}

/// Delete `account_id`, sending its NEAR balance to `beneficiary_id`.
pub async fn delete_account(
    near: &Client,
    signer: &Signer,
    account_id: &AccountId,
    beneficiary_id: &AccountId,
) -> anyhow::Result<()> {
    send_tx_checked(
        near,
        signer,
        account_id,
        vec![Action::DeleteAccount(DeleteAccountAction {
            beneficiary_id: beneficiary_id.clone(),
        })],
    )
    .await
    .with_context(|| format!("delete account {account_id}"))?;

    Ok(())
}

#[must_use]
pub fn deploy_action(code: &[u8]) -> Action {
    Action::DeployContract(DeployContractAction {
        code: code.to_vec(),
    })
}

pub fn require_success_status(outcome: &FinalExecutionOutcomeView) -> anyhow::Result<()> {
    match &outcome.status {
        FinalExecutionStatus::SuccessValue(_) => Ok(()),
        FinalExecutionStatus::Failure(error) => anyhow::bail!("Transaction failed: {error:?}"),
        status => anyhow::bail!("Unexpected transaction status: {status:?}"),
    }
}

async fn view_access_key_with_retry(
    near: &Client,
    signer: &Signer,
) -> anyhow::Result<RpcQueryResponse> {
    let request = || RpcQueryRequest {
        block_reference: BlockReference::latest(),
        request: QueryRequest::ViewAccessKey {
            account_id: signer.get_account_id(),
            public_key: signer.public_key(),
        },
    };

    for attempt in 1..=ACCESS_KEY_LOOKUP_RETRIES {
        match near.call(request()).await {
            Ok(response) => return Ok(response),
            Err(error) if attempt < ACCESS_KEY_LOOKUP_RETRIES => {
                tracing::debug!(
                    account_id = %signer.get_account_id(),
                    public_key = %signer.public_key(),
                    attempt,
                    retries = ACCESS_KEY_LOOKUP_RETRIES,
                    %error,
                    "Retrying access key lookup"
                );
                sleep(Duration::from_millis(
                    ACCESS_KEY_LOOKUP_DELAY_MS * u64::from(attempt),
                ))
                .await;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("fetch access key for {}", signer.get_account_id()));
            }
        }
    }

    unreachable!("retry loop must return or error")
}
