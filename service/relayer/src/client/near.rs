use std::sync::{atomic::AtomicUsize, Arc};

use near_crypto::{PublicKey, Signer};
use near_jsonrpc_client::{
    errors::JsonRpcError,
    methods::{self, gas_price::RpcGasPriceError, query::RpcQueryError, tx::RpcTransactionError},
    JsonRpcClient,
};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::{
    action::{delegate::SignedDelegateAction, Action},
    hash::CryptoHash,
    transaction::{SignedTransaction, Transaction, TransactionV0},
    types::Finality,
    views::{FinalExecutionOutcomeView, QueryRequest},
};
use near_sdk::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json::{self, json},
    AccountId, NearToken,
};
use templar_common::market::MarketConfiguration;

use crate::MarketAccounts;

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
    ) -> Result<Option<FinalExecutionOutcomeView>, JsonRpcError<RpcTransactionError>> {
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

        Ok(response.final_execution_outcome.map(|o| o.into_outcome()))
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
    pub async fn construct_delegate_transaction(
        &self,
        signed_delegate_action: SignedDelegateAction,
    ) -> Result<SignedTransaction, JsonRpcError<RpcQueryError>> {
        let delegate_receiver_id = signed_delegate_action.delegate_action.sender_id.clone();
        let actions = vec![Action::Delegate(Box::new(signed_delegate_action))];
        let signer = self.next_signer();
        let public_key = signer.public_key();

        let (nonce, block_hash) = self
            .fetch_nonce(self.account_id.clone(), public_key.clone())
            .await?;

        Ok(Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key,
            nonce: nonce + 1,
            receiver_id: delegate_receiver_id,
            block_hash,
            actions,
        })
        .sign(signer))
    }

    /// # Errors
    ///
    /// - RPC errors
    pub async fn send_transaction(
        &self,
        signed_transaction: SignedTransaction,
    ) -> Result<near_primitives::views::FinalExecutionOutcomeView, JsonRpcError<RpcTransactionError>>
    {
        self.client
            .call(methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction })
            .await
    }

    async fn view<T: DeserializeOwned>(
        &self,
        account_id: AccountId,
        method_name: impl Into<String>,
        args: &impl Serialize,
    ) -> Result<T, ViewError> {
        let result = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: Finality::Final.into(),
                request: QueryRequest::CallFunction {
                    account_id,
                    method_name: method_name.into(),
                    args: serde_json::to_vec(args)?.into(),
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
        registry_id: &AccountId,
    ) -> Result<Vec<AccountId>, ViewError> {
        self.view(registry_id.clone(), "list_deployments", &json!({}))
            .await
    }

    /// # Errors
    ///
    /// - Serialization/deserialization errors
    /// - RPC errors
    pub async fn load_market_accounts(
        &self,
        market_id: &AccountId,
    ) -> Result<MarketAccounts, ViewError> {
        let market_configuration = self
            .view::<MarketConfiguration>(market_id.clone(), "get_configuration", &json!({}))
            .await?;

        Ok(MarketAccounts {
            account_id: market_id.clone(),
            borrow_asset: market_configuration.borrow_asset,
            collateral_asset: market_configuration.collateral_asset,
        })
    }
}
