use std::sync::{atomic::AtomicUsize, Arc};

use near_crypto::{PublicKey, Signer};
use near_jsonrpc_client::{
    errors::{JsonRpcError, JsonRpcServerError},
    methods::{
        self,
        block::RpcBlockError,
        gas_price::RpcGasPriceError,
        query::RpcQueryError,
        tx::{RpcTransactionError, RpcTransactionResponse},
    },
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{delegate::SignedDelegateAction, FunctionCallAction},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::{BlockId, BlockReference, Finality},
    views::{FinalExecutionOutcomeView, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    json_types::Base64VecU8,
    serde::{de::DeserializeOwned, Deserialize, Serialize},
    serde_json::{self, json},
    AccountId, AccountIdRef, Gas, NearToken,
};
use near_sdk_contract_tools::standard::nep145::{StorageBalance, StorageBalanceBounds};

use templar_universal_account::{KeyId, KeyParameters, PayloadExecutionParameters};

use crate::cache::Cache;

pub const STORAGE_DEPOSIT_GAS: Gas = Gas::from_tgas(5);
pub const DEPLOY_GAS: Gas = Gas::from_tgas(50);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct DeployArgs {
    name: String,
    version_key: String,
    init_args: Base64VecU8,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_access_keys: Option<Vec<near_sdk::PublicKey>>,
}

impl DeployArgs {
    /// # Panics
    ///
    /// - On `init_args` serialization error.
    #[allow(clippy::unwrap_used)]
    pub fn new(
        name: String,
        version_key: String,
        init_args: &impl Serialize,
        full_access_keys: Option<Vec<near_sdk::PublicKey>>,
    ) -> Self {
        Self {
            name,
            version_key,
            init_args: Base64VecU8(near_sdk::serde_json::to_vec(init_args).unwrap()),
            full_access_keys,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Near {
    client: JsonRpcClient,
    account_id: AccountId,
    signers: Arc<Vec<Signer>>,
    signer_ix: Arc<AtomicUsize>,
}

#[derive(Debug, thiserror::Error)]
pub enum NearError {
    #[error("Rpc error: {0}")]
    RpcError(#[from] Box<dyn std::error::Error>),
    #[error("Parse error: {0}")]
    ParseError(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("Rpc error: {0}")]
    Rpc(#[from] JsonRpcError<RpcQueryError>),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl ViewError {
    pub fn is_method_not_found(&self) -> bool {
        matches!(
            self,
            ViewError::Rpc(JsonRpcError::ServerError(JsonRpcServerError::HandlerError(
                RpcQueryError::ContractExecutionError { vm_error, .. }
            ))) if vm_error.contains("MethodNotFound")
        )
    }
}

#[derive(Debug, Clone)]
pub struct FetchNonce {
    pub nonce: u64,
    pub block_height: u64,
    pub block_hash: CryptoHash,
}

#[allow(clippy::unwrap_used)]
impl Near {
    pub fn new(client: JsonRpcClient, account_id: AccountId, signers: Vec<Signer>) -> Self {
        Self {
            client,
            account_id,
            signers: Arc::new(signers),
            signer_ix: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self), name = "fetch_gas_price")]
    pub async fn fetch_gas_price(&self) -> Result<NearToken, JsonRpcError<RpcGasPriceError>> {
        let method = methods::gas_price::RpcGasPriceRequest { block_id: None };
        let response = self.client.call(method).await?;
        tracing::trace!(gas_price = %response.gas_price, "Fetched gas price");
        Ok(response.gas_price)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self))]
    pub async fn fetch_protocol_config(
        &self,
    ) -> Result<methods::EXPERIMENTAL_protocol_config::RpcProtocolConfigResponse, NearError> {
        let method = methods::EXPERIMENTAL_protocol_config::RpcProtocolConfigRequest {
            block_reference: BlockReference::latest(),
        };

        match self.client.call(method).await {
            Ok(response) => {
                tracing::trace!(protocol_config = ?response, "Fetched protocol config");
                Ok(response)
            }
            Err(error) => {
                tracing::warn!(%error, "Protocol config RPC deserialization failed");
                Err(NearError::RpcError(Box::new(error)))
            }
        }
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_block_timestamp_ms(
        &self,
        block_hash: CryptoHash,
    ) -> Result<u64, JsonRpcError<RpcBlockError>> {
        let response = self
            .client
            .call(methods::block::RpcBlockRequest {
                block_reference: BlockId::Hash(block_hash).into(),
            })
            .await?;

        Ok(response.header.timestamp_nanosec / 1_000_000)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self), fields(account_id = %account_id, transaction_hash = %transaction_hash))]
    pub async fn fetch_transaction_status(
        &self,
        account_id: AccountId,
        transaction_hash: CryptoHash,
    ) -> Result<FinalExecutionOutcomeView, JsonRpcError<RpcTransactionError>> {
        tracing::debug!("Fetching transaction status");
        let response = self
            .client
            .call(methods::tx::RpcTransactionStatusRequest {
                transaction_info: methods::tx::TransactionInfo::TransactionId {
                    tx_hash: transaction_hash,
                    sender_account_id: account_id,
                },
                wait_until: near_primitives::views::TxExecutionStatus::Final,
            })
            .await?;

        #[allow(
            clippy::unwrap_used,
            reason = "TxExecutionStatus::Final guarantees outcome is not None"
        )]
        let outcome = response.final_execution_outcome.unwrap().into_outcome();
        tracing::debug!(status = ?outcome.status, "Transaction status fetched");
        Ok(outcome)
    }

    pub fn next_signer(&self) -> &Signer {
        use std::sync::atomic::Ordering;
        let i = match self
            .signer_ix
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |ix| {
                Some((ix + 1) % self.signers.len())
            }) {
            Ok(i) | Err(i) => i,
        };
        &self.signers[i]
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_nonce(
        &self,
        account_id: AccountId,
        public_key: PublicKey,
    ) -> Result<FetchNonce, JsonRpcError<RpcQueryError>> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::ViewAccessKey {
                    account_id,
                    public_key,
                },
            })
            .await?;

        let QueryResponseKind::AccessKey(access_key) = response.kind else {
            unimplemented!("Invalid response kind");
        };

        Ok(FetchNonce {
            nonce: access_key.nonce,
            block_hash: response.block_hash,
            block_height: response.block_height,
        })
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_near_balance(
        &self,
        account_id: AccountId,
    ) -> Result<NearToken, JsonRpcError<RpcQueryError>> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::ViewAccount { account_id },
            })
            .await?;

        let QueryResponseKind::ViewAccount(account) = response.kind else {
            unreachable!("Invalid response kind");
        };

        Ok(account.amount.saturating_sub(account.locked))
    }

    /// # Errors
    ///
    /// - RPC errors for nonce query
    #[must_use]
    pub async fn construct_delegate_transaction(
        &self,
        cache: &Cache,
        signed_delegate_action: SignedDelegateAction,
    ) -> SignedTransaction {
        let delegate_receiver_id = signed_delegate_action.delegate_action.sender_id.clone();
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: delegate_receiver_id,
            block_hash,
            actions: vec![signed_delegate_action.into()],
        })
        .sign(signer)
    }

    /// Constructs a storage deposit transaction for the given account and contract.
    ///
    /// # Errors
    ///
    /// - RPC transaction error
    #[must_use]
    pub async fn construct_storage_deposit_transaction(
        &self,
        cache: &Cache,
        account_id: AccountId,
        contract_id: AccountId,
        amount: NearToken,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: serde_json::to_vec(&json!({
                "account_id": account_id,
            }))
            .unwrap(),
            gas: near_primitives::gas::Gas::from_gas(STORAGE_DEPOSIT_GAS.as_gas()),
            deposit: amount,
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: contract_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    /// Deploy a version of a contract from a registry.
    #[must_use]
    pub async fn construct_deploy_from_registry_transaction(
        &self,
        cache: &Cache,
        registry_id: AccountId,
        args: &DeployArgs,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "deploy".to_string(),
            args: serde_json::to_vec(args).unwrap(),
            gas: near_primitives::gas::Gas::from_gas(DEPLOY_GAS.as_gas()),
            deposit: NearToken::ZERO,
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: registry_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    #[must_use]
    pub async fn construct_ua_execute_transaction(
        &self,
        cache: &Cache,
        ua_account_id: AccountId,
        args: &serde_json::Value,
        gas: near_primitives::gas::Gas,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        let action = FunctionCallAction {
            method_name: "execute".to_string(),
            args: serde_json::to_vec(&json!({ "args": args })).unwrap(),
            gas,
            deposit: NearToken::ZERO,
        };

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id: ua_account_id,
            block_hash,
            actions: vec![action.into()],
        })
        .sign(signer)
    }

    #[must_use]
    pub async fn sign_transaction(
        &self,
        cache: &Cache,
        receiver_id: AccountId,
        actions: Vec<near_primitives::action::Action>,
    ) -> SignedTransaction {
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = cache
            .nonce(self.account_id.clone(), public_key.clone())
            .await;

        Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce,
            receiver_id,
            block_hash,
            actions,
        })
        .sign(signer)
    }

    /// # Errors
    ///
    /// - RPC errors
    #[tracing::instrument(skip(self, signed_transaction), fields(
        transaction_hash = %signed_transaction.get_hash(),
        wait_until = ?wait_until
    ))]
    pub async fn send_transaction(
        &self,
        signed_transaction: SignedTransaction,
        wait_until: TxExecutionStatus,
    ) -> Result<RpcTransactionResponse, JsonRpcError<RpcTransactionError>> {
        tracing::info!("Sending transaction to NEAR");
        let result = self
            .client
            .call(methods::send_tx::RpcSendTransactionRequest {
                signed_transaction,
                wait_until,
            })
            .await;

        match &result {
            Ok(_) => tracing::info!("Transaction sent successfully"),
            Err(e) => tracing::error!(error = %e, "Failed to send transaction"),
        }

        result
    }

    pub async fn view_raw(
        &self,
        account_id: AccountId,
        method_name: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, JsonRpcError<RpcQueryError>> {
        let result = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::CallFunction {
                    account_id,
                    method_name,
                    args: args.into(),
                },
            })
            .await?;

        let QueryResponseKind::CallResult(result) = result.kind else {
            unimplemented!("Invalid response kind");
        };

        Ok(result.result)
    }

    pub async fn view<T: DeserializeOwned>(
        &self,
        account_id: AccountId,
        method_name: impl Into<String>,
        args: impl Serialize,
    ) -> Result<T, ViewError> {
        let raw_result = self
            .view_raw(account_id, method_name.into(), serde_json::to_vec(&args)?)
            .await?;

        Ok(serde_json::from_slice(&raw_result)?)
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_storage_balance_bounds(
        &self,
        contract_id: AccountId,
    ) -> Result<StorageBalanceBounds, ViewError> {
        self.view(contract_id, "storage_balance_bounds", json!({}))
            .await
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_storage_balance_of(
        &self,
        contract_id: AccountId,
        account_id: &AccountIdRef,
    ) -> Result<Option<StorageBalance>, ViewError> {
        self.view(
            contract_id,
            "storage_balance_of",
            &json!({ "account_id": account_id }),
        )
        .await
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn load_ua_key(
        &self,
        ua_account_id: AccountId,
        key: KeyId,
    ) -> Result<Option<PayloadExecutionParameters>, ViewError> {
        let view = self
            .view::<Option<VersionedKeyParameters>>(
                ua_account_id.clone(),
                "get_key",
                json!({ "key": key }),
            )
            .await?;

        Ok(view.map(|v| match v {
            VersionedKeyParameters::V1(p) => p,
            VersionedKeyParameters::V0(p) => PayloadExecutionParameters::builder_empty()
                .with_key_parameters(p)
                .verifying_contract(ua_account_id)
                .build(),
        }))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub enum VersionedKeyParameters {
    #[serde(untagged)]
    V1(PayloadExecutionParameters),
    #[serde(untagged)]
    V0(KeyParameters),
}
