use std::sync::Arc;

use async_trait::async_trait;
use near_api::NetworkConfig;
use near_api::{
    advanced::{
        tx_rpc::{TransactionStatusRef, TransactionStatusRpc},
        ExecuteSignedTransaction, RequestBuilder, TransactionStatusHandler,
        TransactionableOrSigned,
    },
    Signer,
};
use near_api::{
    types::{
        crypto::secret_key::ED25519SecretKey,
        transaction::{
            result::{ExecutionFinalResult, TransactionResult},
            PrepopulateTransaction, SignedTransaction,
        },
    },
    SecretKey,
};
use std::collections::HashMap;

use templar_gateway_types::{CryptoHash, ManagedAccountId};

use crate::{
    read::is_unknown_transaction, GatewayError, GatewayResult, PlannedTransaction,
    PreparedTransactionResult,
};

pub type SharedExecuteOperation = Arc<dyn ExecuteOperation>;
pub type SharedSignTransaction = Arc<dyn SignTransaction>;

#[async_trait]
pub trait SignTransaction: Send + Sync {
    async fn sign_transaction(
        &self,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult>;
}

#[async_trait]
pub trait ExecuteOperation: Send + Sync {
    async fn submit_transaction(
        &self,
        signed_transaction: SignedTransaction,
        wait_until: templar_gateway_types::common::TxExecutionStatus,
    ) -> GatewayResult<TransactionResult>;

    async fn query_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<ExecutionFinalResult>;
}

#[derive(Clone)]
pub struct NearTransactionSigner {
    network: NetworkConfig,
    signers: HashMap<ManagedAccountId, Arc<near_api::Signer>>,
}

impl NearTransactionSigner {
    pub fn new(
        network: NetworkConfig,
        signers: HashMap<ManagedAccountId, Arc<near_api::Signer>>,
    ) -> Self {
        Self { network, signers }
    }

    fn signer_for(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<Arc<near_api::Signer>> {
        self.signers
            .get(signer_account_id)
            .cloned()
            .ok_or_else(|| GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string()))
    }
}

#[derive(Clone)]
pub struct NearOperationExecutor {
    network: NetworkConfig,
}

impl NearOperationExecutor {
    pub fn new(network: NetworkConfig) -> Self {
        Self { network }
    }
}

#[derive(Debug, Clone)]
struct PrepopulatedTransactionCarrier(PrepopulateTransaction);

#[async_trait]
impl near_api::advanced::Transactionable for PrepopulatedTransactionCarrier {
    fn prepopulated(
        &self,
    ) -> Result<PrepopulateTransaction, near_api::errors::ArgumentValidationError> {
        Ok(self.0.clone())
    }

    async fn validate_with_network(
        &self,
        _network: &near_api::NetworkConfig,
    ) -> Result<(), near_api::errors::ValidationError> {
        Ok(())
    }
}

#[async_trait]
impl SignTransaction for NearTransactionSigner {
    async fn sign_transaction(
        &self,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        let signer = self.signer_for(&transaction.signer_account_id)?;
        let presigned = near_api::Transaction::use_transaction(
            PrepopulateTransaction {
                signer_id: transaction.signer_account_id.0.clone(),
                receiver_id: transaction.receiver_id.clone(),
                actions: transaction.actions.clone(),
            },
            signer,
        )
        .wait_until(transaction.wait_until.into())
        .presign_with(&self.network)
        .await
        .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

        let Some(signed_transaction) = presigned.transaction.signed() else {
            return Err(GatewayError::NearTransaction(
                "failed to extract presigned transaction".to_owned(),
            ));
        };
        let tx_hash = signed_transaction.get_hash().into();

        Ok(PreparedTransactionResult {
            transaction,
            tx_hash,
            signed_transaction,
        })
    }
}

#[async_trait]
impl ExecuteOperation for NearOperationExecutor {
    async fn submit_transaction(
        &self,
        signed_transaction: SignedTransaction,
        wait_until: templar_gateway_types::common::TxExecutionStatus,
    ) -> GatewayResult<TransactionResult> {
        let prepopulated = PrepopulateTransaction {
            signer_id: signed_transaction.transaction.signer_id().clone(),
            receiver_id: signed_transaction.transaction.receiver_id().clone(),
            actions: signed_transaction.transaction.actions().to_vec(),
        };

        ExecuteSignedTransaction {
            transaction: TransactionableOrSigned::Signed((
                signed_transaction,
                Box::new(PrepopulatedTransactionCarrier(prepopulated)),
            )),
            signer: null_signer(),
            wait_until: wait_until.into(),
        }
        .send_to(&self.network)
        .await
        .map_err(|error| GatewayError::NearTransaction(error.to_string()))
    }

    async fn query_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<ExecutionFinalResult> {
        RequestBuilder::new(
            TransactionStatusRpc,
            TransactionStatusRef {
                sender_account_id: signer_account_id.0.clone(),
                tx_hash: tx_hash.0,
                wait_until: near_api::types::TxExecutionStatus::Final,
            },
            TransactionStatusHandler,
        )
        .fetch_from(&self.network)
        .await
        // Classify "the chain has no record of this transaction" at the boundary
        // (on the raw error) into a typed variant, so reconciliation can tell a
        // never-landed transaction from a transient query failure.
        .map_err(|error| {
            if is_unknown_transaction(&error) {
                GatewayError::TransactionNotFound
            } else {
                GatewayError::NearTransaction(error.to_string())
            }
        })
    }
}

#[allow(
    clippy::unwrap_used,
    reason = "zeroed ED25519 secret key is locally constructed and should always parse"
)]
fn null_signer() -> Arc<near_api::Signer> {
    Signer::from_secret_key(SecretKey::ED25519(ED25519SecretKey::from_secret_key(
        [0; 32],
    )))
    .unwrap()
}
