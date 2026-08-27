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

    /// Look up an already-submitted transaction by hash.
    async fn query_transaction(
        &self,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<TransactionRecord>;
}

/// What the chain says about a submitted transaction. A failure to *reach* the
/// chain must be `Err`, never [`TransactionRecord::NoRecord`]: past a
/// transaction's validity horizon reconciliation treats `NoRecord` as proof it
/// never landed, and rejects the step.
pub enum TransactionRecord {
    /// The chain reported an outcome for the transaction.
    Executed(StepOutcome),
    /// Every endpoint queried agreed the chain has no record of the transaction.
    NoRecord,
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
    /// One single-endpoint view of `network` per endpoint, so a status query can
    /// be addressed to each in turn. Retries are stripped: near_api treats
    /// `UNKNOWN_TRANSACTION` as retryable and would sleep through its whole
    /// backoff schedule before the answer could be classified, on every orphan of
    /// every recovery sweep.
    status_query_networks: Vec<NetworkConfig>,
    finality_policy: FinalityPolicy,
}

impl NearOperationExecutor {
    pub fn new(network: NetworkConfig) -> Self {
        Self::with_finality_policy(network, FinalityPolicy::default())
    }

    pub fn with_finality_policy(network: NetworkConfig, finality_policy: FinalityPolicy) -> Self {
        let status_query_networks = network
            .rpc_endpoints
            .iter()
            .map(|endpoint| NetworkConfig {
                rpc_endpoints: vec![endpoint.clone().with_retries(1)],
                ..network.clone()
            })
            .collect();
        Self {
            network,
            status_query_networks,
            finality_policy,
        }
    }

    async fn query_transaction_from(
        &self,
        network: &NetworkConfig,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<TransactionRecord> {
        match RequestBuilder::new(
            TransactionStatusRpc,
            TransactionStatusRef {
                sender_account_id: signer_account_id.0.clone(),
                tx_hash: tx_hash.0,
                wait_until: self.finality_policy.transaction_status(),
            },
            TransactionStatusHandler,
        )
        .fetch_from(network)
        .await
        {
            Ok(result) => Ok(TransactionRecord::Executed(StepOutcome::from_execution(
                result,
            ))),
            Err(error) if is_unknown_transaction(&error) => Ok(TransactionRecord::NoRecord),
            Err(error) => Err(GatewayError::NearTransaction(error.to_string())),
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

    async fn sign_transaction(
        &self,
        lease: &SigningKeyLease,
        transaction: PlannedTransaction,
    ) -> GatewayResult<PreparedTransactionResult> {
        // The lease signs as its own account, so a step leased against a
        // different one would be recorded under an account that did not sign it.
        if transaction.signer_account_id != *lease.account_id() {
            return Err(GatewayError::UnsupportedSignerAccount(format!(
                "{} cannot sign for {}",
                lease.account_id().0,
                transaction.signer_account_id.0
            )));
        }

        let signed_transaction = lease
            .presign(
                &self.network,
                transaction.receiver_id.clone(),
                transaction.actions.clone(),
            )
            .await?;
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
    ) -> GatewayResult<TransactionRecord> {
        // near_api walks the endpoints itself, but collapses the walk into a
        // single last error — so one endpoint's "no record" can be reported while
        // an earlier one never answered at all. Walking them here keeps the two
        // apart: an endpoint that failed to respond outranks another's `NoRecord`.
        if self.status_query_networks.is_empty() {
            return Err(GatewayError::NearQuery(
                "no RPC endpoint available for transaction status".to_owned(),
            ));
        }

        let mut unanswered = None;
        for network in &self.status_query_networks {
            match self
                .query_transaction_from(network, signer_account_id, tx_hash)
                .await
            {
                Ok(executed @ TransactionRecord::Executed(_)) => return Ok(executed),
                Ok(TransactionRecord::NoRecord) => {}
                Err(error) => unanswered = Some(error),
            }
        }
        unanswered.map_or(Ok(TransactionRecord::NoRecord), Err)
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

#[cfg(test)]
mod tests {
    use near_api::{NetworkConfig, RPCEndpoint};
    use templar_gateway_types::{CryptoHash, ManagedAccountId};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    use super::{ExecuteOperation, NearOperationExecutor, TransactionRecord};
    use crate::GatewayError;

    const UNKNOWN_TRANSACTION: &str = r#"{"jsonrpc":"2.0","id":"0","error":{"name":"HANDLER_ERROR","cause":{"name":"UNKNOWN_TRANSACTION"}}}"#;
    const INTERNAL_ERROR: &str = r#"{"jsonrpc":"2.0","id":"0","error":{"name":"INTERNAL_ERROR","info":{"error_message":"unavailable"}}}"#;

    async fn responding_with(body: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.to_owned()))
            .mount(&server)
            .await;
        server
    }

    /// Endpoints are walked in order, so the reply comes from the last one
    /// consulted rather than the first.
    fn executor_over(servers: &[&MockServer]) -> NearOperationExecutor {
        let mut network = NetworkConfig::from_rpc_url("test", servers[0].uri().parse().unwrap());
        network.rpc_endpoints = servers
            .iter()
            .map(|server| RPCEndpoint::new(server.uri().parse().unwrap()))
            .collect();
        NearOperationExecutor::new(network)
    }

    async fn query(executor: &NearOperationExecutor) -> crate::GatewayResult<TransactionRecord> {
        executor
            .query_transaction(
                &ManagedAccountId("signer.near".parse().unwrap()),
                CryptoHash(near_api::types::CryptoHash::default()),
            )
            .await
    }

    #[tokio::test]
    async fn every_endpoint_agreeing_on_no_record_is_an_answer() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;
        let archival = responding_with(UNKNOWN_TRANSACTION).await;

        let record = query(&executor_over(&[&primary, &archival])).await.unwrap();

        assert!(matches!(record, TransactionRecord::NoRecord));
    }

    /// The safety property the horizon rule rests on: an endpoint that failed to
    /// answer leaves the question open, so its silence must not be reported as
    /// the chain having no record — that would reject a transaction which may
    /// well have executed.
    #[tokio::test]
    async fn an_unreachable_endpoint_is_an_error_not_an_answer() {
        let unreachable = responding_with(INTERNAL_ERROR).await;
        let archival = responding_with(UNKNOWN_TRANSACTION).await;

        let result = query(&executor_over(&[&unreachable, &archival])).await;

        assert!(
            matches!(result, Err(GatewayError::NearTransaction(_))),
            "an unanswered endpoint must not resolve to NoRecord"
        );
    }

    /// With nothing to ask, there is no answer — reporting `NoRecord` here would
    /// let reconciliation reject a transaction nobody looked for.
    #[tokio::test]
    async fn a_configuration_with_no_endpoints_is_an_error_not_an_answer() {
        let mut network =
            NetworkConfig::from_rpc_url("test", "http://127.0.0.1:1".parse().unwrap());
        network.rpc_endpoints.clear();

        let result = query(&NearOperationExecutor::new(network)).await;

        assert!(
            matches!(result, Err(GatewayError::NearQuery(_))),
            "an unasked question must not resolve to NoRecord"
        );
    }

    #[tokio::test]
    async fn a_lone_endpoint_reporting_no_record_is_an_answer() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;

        let record = query(&executor_over(&[&primary])).await.unwrap();

        assert!(matches!(record, TransactionRecord::NoRecord));
    }
}
