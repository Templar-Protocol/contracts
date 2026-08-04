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
    PublicKey, SecretKey,
};
use std::collections::HashMap;

use templar_gateway_types::{operation::ExecutionOutcome, CryptoHash, ManagedAccountId};

use crate::{
    read::is_unknown_transaction, FinalityPolicy, GatewayError, GatewayResult, PlannedTransaction,
    PooledSigner, PreparedTransactionResult, SigningKeyLease,
};

pub type SharedExecuteOperation = Arc<dyn ExecuteOperation>;
pub type SharedSignTransaction = Arc<dyn SignTransaction>;

#[async_trait]
pub trait SignTransaction: Send + Sync {
    /// Lease the account's next idle access key.
    async fn lease_next_signing_key(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<SigningKeyLease>;

    /// Lease the specific key a transaction was already signed with, or `None`
    /// when this process holds no such key.
    async fn lease_signing_key(
        &self,
        signer_account_id: &ManagedAccountId,
        public_key: &PublicKey,
    ) -> Option<SigningKeyLease>;

    /// Signs with `lease`'s key and allocates that key's nonce, so the caller
    /// must hold `lease` at least until the signed transaction has been
    /// broadcast.
    async fn sign_transaction(
        &self,
        lease: &SigningKeyLease,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult>;
}

/// The chain-side result of a single operation step: the executing transaction's
/// hash, whether it succeeded, and its captured outcome. Isolating this behind
/// the trait keeps near-api's result shapes out of the driver, and lets tests
/// drive an operation without a live chain.
pub struct StepOutcome {
    pub tx_hash: CryptoHash,
    pub is_success: bool,
    pub outcome: ExecutionOutcome,
}

impl StepOutcome {
    fn from_execution(result: ExecutionFinalResult) -> Self {
        Self {
            tx_hash: result.outcome().transaction_hash.into(),
            is_success: result.is_success(),
            outcome: ExecutionOutcome::from(result),
        }
    }
}

#[async_trait]
pub trait ExecuteOperation: Send + Sync {
    /// Submit a signed transaction, waiting for the configured complete
    /// execution status. `Ok(None)` means it was broadcast but no full outcome
    /// is available yet (still in flight); `Ok(Some)` carries the result.
    async fn submit_transaction(
        &self,
        signed_transaction: SignedTransaction,
    ) -> GatewayResult<Option<StepOutcome>>;

    /// Look up an already-submitted transaction by hash, returning
    /// [`GatewayError::TransactionNotFound`] when the chain has no record of it.
    async fn query_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<StepOutcome>;
}

#[derive(Clone)]
pub struct NearTransactionSigner {
    network: NetworkConfig,
    /// Shared so clones lease against the same key slots.
    signers: Arc<HashMap<ManagedAccountId, PooledSigner>>,
}

impl NearTransactionSigner {
    pub fn new(network: NetworkConfig, signers: HashMap<ManagedAccountId, PooledSigner>) -> Self {
        Self {
            network,
            signers: Arc::new(signers),
        }
    }

    fn signer_for(&self, signer_account_id: &ManagedAccountId) -> GatewayResult<&PooledSigner> {
        self.signers
            .get(signer_account_id)
            .ok_or_else(|| GatewayError::UnsupportedSignerAccount(signer_account_id.0.to_string()))
    }
}

#[derive(Clone)]
pub struct NearOperationExecutor {
    network: NetworkConfig,
    finality_policy: FinalityPolicy,
}

impl NearOperationExecutor {
    pub fn new(network: NetworkConfig) -> Self {
        Self::with_finality_policy(network, FinalityPolicy::default())
    }

    pub fn with_finality_policy(network: NetworkConfig, finality_policy: FinalityPolicy) -> Self {
        Self {
            network,
            finality_policy,
        }
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
    async fn lease_next_signing_key(
        &self,
        signer_account_id: &ManagedAccountId,
    ) -> GatewayResult<SigningKeyLease> {
        Ok(self.signer_for(signer_account_id)?.lease_next().await)
    }

    async fn lease_signing_key(
        &self,
        signer_account_id: &ManagedAccountId,
        public_key: &PublicKey,
    ) -> Option<SigningKeyLease> {
        self.signer_for(signer_account_id)
            .ok()?
            .lease(public_key)
            .await
    }

    async fn sign_transaction(
        &self,
        lease: &SigningKeyLease,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        let signer = lease.signer();
        // Naming the key keeps the signature on the leased lane; `presign_with`
        // would instead take whichever key near-api's rotation hands back.
        let public_key = lease.public_key();
        let (nonce, block_hash, _) = signer
            .fetch_tx_nonce(
                transaction.signer_account_id.0.clone(),
                public_key,
                &self.network,
            )
            .await
            .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

        let presigned = near_api::Transaction::use_transaction(
            PrepopulateTransaction {
                signer_id: transaction.signer_account_id.0.clone(),
                receiver_id: transaction.receiver_id.clone(),
                actions: transaction.actions.clone(),
            },
            signer,
        )
        .presign_offline(public_key, block_hash, nonce)
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
    ) -> GatewayResult<Option<StepOutcome>> {
        let prepopulated = PrepopulateTransaction {
            signer_id: signed_transaction.transaction.signer_id().clone(),
            receiver_id: signed_transaction.transaction.receiver_id().clone(),
            actions: signed_transaction.transaction.actions().to_vec(),
        };

        let result: TransactionResult = ExecuteSignedTransaction {
            transaction: TransactionableOrSigned::Signed((
                signed_transaction,
                Box::new(PrepopulatedTransactionCarrier(prepopulated)),
            )),
            signer: null_signer(),
            wait_until: self.finality_policy.transaction_status(),
        }
        .send_to(&self.network)
        .await
        .map_err(|error| GatewayError::NearTransaction(error.to_string()))?;

        Ok(result.into_full().map(StepOutcome::from_execution))
    }

    async fn query_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<StepOutcome> {
        RequestBuilder::new(
            TransactionStatusRpc,
            TransactionStatusRef {
                sender_account_id: signer_account_id.0.clone(),
                tx_hash: tx_hash.0,
                wait_until: self.finality_policy.transaction_status(),
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
        .map(StepOutcome::from_execution)
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
