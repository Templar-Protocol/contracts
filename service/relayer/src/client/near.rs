use std::sync::{atomic::AtomicUsize, Arc};

use near_crypto::{PublicKey, Signer};
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{
        self,
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
    types::Finality,
    views::{FinalExecutionOutcomeView, QueryRequest, TxExecutionStatus},
};
use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json::{self, json},
    AccountId, AccountIdRef, Gas, NearToken,
};
use near_sdk_contract_tools::standard::nep145::{StorageBalance, StorageBalanceBounds};

use templar_common::market::MarketConfiguration;

use crate::{cache::Cache, MarketData};

pub const STORAGE_DEPOSIT_GAS: u64 = Gas::from_tgas(5).as_gas();

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
    pub async fn fetch_gas_price(&self) -> Result<NearToken, JsonRpcError<RpcGasPriceError>> {
        let method = methods::gas_price::RpcGasPriceRequest { block_id: None };
        let response = self.client.call(method).await?;
        Ok(NearToken::from_yoctonear(response.gas_price))
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn fetch_transaction_status(
        &self,
        account_id: AccountId,
        transaction_hash: CryptoHash,
    ) -> Result<FinalExecutionOutcomeView, JsonRpcError<RpcTransactionError>> {
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
        Ok(response.final_execution_outcome.unwrap().into_outcome())
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
    ) -> Result<(u64, CryptoHash), JsonRpcError<RpcQueryError>> {
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

        Ok((access_key.nonce, response.block_hash))
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
            unimplemented!("Invalid response kind");
        };

        Ok(NearToken::from_yoctonear(
            account.amount.saturating_sub(account.locked),
        ))
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
                "registration_only": true,
            }))
            .unwrap(),
            gas: STORAGE_DEPOSIT_GAS,
            deposit: amount.as_yoctonear(),
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

    /// # Errors
    ///
    /// - RPC errors
    pub async fn send_transaction(
        &self,
        signed_transaction: SignedTransaction,
        wait_until: TxExecutionStatus,
    ) -> Result<RpcTransactionResponse, JsonRpcError<RpcTransactionError>> {
        self.client
            .call(methods::send_tx::RpcSendTransactionRequest {
                signed_transaction,
                wait_until,
            })
            .await
    }

    async fn view<T: DeserializeOwned>(
        &self,
        account_id: AccountId,
        method_name: impl Into<String>,
        args: impl Serialize,
    ) -> Result<T, ViewError> {
        let result = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::CallFunction {
                    account_id,
                    method_name: method_name.into(),
                    args: serde_json::to_vec(&args)?.into(),
                },
            })
            .await?;

        let QueryResponseKind::CallResult(result) = result.kind else {
            unimplemented!("Invalid response kind");
        };

        Ok(serde_json::from_slice(&result.result)?)
    }

    /// # Errors
    ///
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_deployments_from_registry(
        &self,
        registry_id: AccountId,
    ) -> Result<Vec<AccountId>, ViewError> {
        self.view(registry_id, "list_deployments", json!({})).await
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
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_market_accounts(
        &self,
        market_id: AccountId,
    ) -> Result<MarketData, ViewError> {
        let market_configuration = self
            .view::<MarketConfiguration>(market_id.clone(), "get_configuration", json!({}))
            .await?;

        Ok(MarketData {
            account_id: market_id.clone(),
            borrow_asset: market_configuration.borrow_asset,
            collateral_asset: market_configuration.collateral_asset,
        })
    }
}
