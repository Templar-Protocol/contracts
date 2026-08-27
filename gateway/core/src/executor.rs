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

/// What the chain says about a submitted transaction.
pub enum TransactionRecord {
    /// The chain reported an outcome for the transaction.
    Executed(StepOutcome),
    /// An archival node — which retains history rather than garbage collecting
    /// it — has no record of the transaction. Only this is evidence that it
    /// never executed, and past its validity horizon reconciliation rejects the
    /// step on it.
    NoRecord,
    /// No node that retains history answered, so absence of a record proves
    /// nothing: a primary node discards outcomes it no longer needs, and reports
    /// a transaction that did execute exactly as it reports one that never did.
    Unconfirmed,
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
    /// Retention-complete view of the chain, when one is configured. Absent, a
    /// missing transaction can never be confirmed missing — see
    /// [`TransactionRecord::Unconfirmed`].
    archival_status_network: Option<NetworkConfig>,
    /// One single-endpoint, single-attempt view of `network` per endpoint, so a
    /// status query can be addressed to each in turn.
    status_query_networks: Vec<NetworkConfig>,
    finality_policy: FinalityPolicy,
}

impl NearOperationExecutor {
    pub fn new(network: NetworkConfig, archival_network: Option<NetworkConfig>) -> Self {
        Self::with_finality_policy(network, archival_network, FinalityPolicy::default())
    }

    /// `archival_network` is required rather than opt-in: it is what lets
    /// reconciliation terminally reject a transaction the chain has no record of,
    /// so a consumer that never considers it silently loses that — and its
    /// operations stay in flight forever. `None` is a deliberate choice to do
    /// without, not an oversight the type permits.
    pub fn with_finality_policy(
        network: NetworkConfig,
        archival_network: Option<NetworkConfig>,
        finality_policy: FinalityPolicy,
    ) -> Self {
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
            archival_status_network: archival_network
                .as_ref()
                .map(|network| Self::status_query(network)),
            finality_policy,
        }
    }

    /// A single-attempt view of `network`. near_api treats `UNKNOWN_TRANSACTION`
    /// as retryable and would sleep through its whole backoff schedule before the
    /// answer could be classified, on every orphan of every recovery sweep.
    fn status_query(network: &NetworkConfig) -> NetworkConfig {
        NetworkConfig {
            rpc_endpoints: network
                .rpc_endpoints
                .iter()
                .map(|endpoint| endpoint.clone().with_retries(1))
                .collect(),
            ..network.clone()
        }
    }

    /// `Ok(None)` when this node has no record of the transaction. Whether that
    /// is evidence of anything depends on whether the node retains history, which
    /// is the caller's to judge.
    async fn transaction_outcome_from(
        &self,
        network: &NetworkConfig,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<Option<StepOutcome>> {
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
            Ok(result) => Ok(Some(StepOutcome::from_execution(result))),
            Err(error) if is_unknown_transaction(&error) => Ok(None),
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
        // near_api walks the endpoints itself but collapses the walk into a single
        // last error, losing which node said what. Walking them here keeps the
        // primaries' answers separate from the archival one that can settle them.
        if self.status_query_networks.is_empty() {
            return Err(GatewayError::NearQuery(
                "no RPC endpoint available for transaction status".to_owned(),
            ));
        }

        let mut unanswered = None;
        for network in &self.status_query_networks {
            match self
                .transaction_outcome_from(network, signer_account_id, tx_hash)
                .await
            {
                Ok(Some(outcome)) => return Ok(TransactionRecord::Executed(outcome)),
                Ok(None) => {}
                Err(error) => unanswered = Some(error),
            }
        }

        let Some(archival) = &self.archival_status_network else {
            return unanswered.map_or(Ok(TransactionRecord::Unconfirmed), Err);
        };

        // The only construction of `NoRecord`, reachable only from the archival
        // branch — which is what keeps a primary node's garbage collection from
        // being mistaken for proof that a transaction never executed.
        Ok(
            match self
                .transaction_outcome_from(archival, signer_account_id, tx_hash)
                .await?
            {
                Some(outcome) => TransactionRecord::Executed(outcome),
                None => TransactionRecord::NoRecord,
            },
        )
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

    async fn requests_to(server: &MockServer) -> usize {
        server.received_requests().await.unwrap_or_default().len()
    }

    fn executor_over(
        servers: &[&MockServer],
        archival: Option<&MockServer>,
    ) -> NearOperationExecutor {
        let network_over = |urls: Vec<&MockServer>| {
            let mut network = NetworkConfig::from_rpc_url("test", urls[0].uri().parse().unwrap());
            network.rpc_endpoints = urls
                .iter()
                .map(|server| RPCEndpoint::new(server.uri().parse().unwrap()))
                .collect();
            network
        };
        NearOperationExecutor::new(
            network_over(servers.to_vec()),
            archival.map(|server| network_over(vec![server])),
        )
    }

    async fn query(executor: &NearOperationExecutor) -> crate::GatewayResult<TransactionRecord> {
        executor
            .query_transaction(
                &ManagedAccountId("signer.near".parse().unwrap()),
                CryptoHash(near_api::types::CryptoHash::default()),
            )
            .await
    }

    /// Absence of a record is only evidence from a node that retains history, so
    /// a primary saying so on its own must not resolve to `NoRecord` — it would
    /// let the horizon rule reject a transaction whose outcome was merely
    /// garbage collected.
    #[tokio::test]
    async fn a_primary_alone_cannot_confirm_a_missing_transaction() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;

        let record = query(&executor_over(&[&primary], None)).await.unwrap();

        assert!(matches!(record, TransactionRecord::Unconfirmed));
    }

    #[tokio::test]
    async fn an_archival_endpoint_confirms_a_missing_transaction() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;
        let archival = responding_with(UNKNOWN_TRANSACTION).await;

        let record = query(&executor_over(&[&primary], Some(&archival)))
            .await
            .unwrap();

        assert!(matches!(record, TransactionRecord::NoRecord));
        assert_eq!(requests_to(&primary).await, 1, "one request per endpoint");
        assert_eq!(requests_to(&archival).await, 1, "one request per endpoint");
    }

    /// An archival endpoint that cannot be reached has confirmed nothing.
    #[tokio::test]
    async fn an_unreachable_archival_endpoint_is_an_error_not_an_answer() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;
        let archival = responding_with(INTERNAL_ERROR).await;

        let result = query(&executor_over(&[&primary], Some(&archival))).await;

        assert!(
            matches!(result, Err(GatewayError::NearTransaction(_))),
            "an unreachable archival endpoint must not resolve to NoRecord"
        );
    }

    #[tokio::test]
    async fn an_unreachable_primary_without_archival_is_an_error() {
        let primary = responding_with(INTERNAL_ERROR).await;

        let result = query(&executor_over(&[&primary], None)).await;

        assert!(matches!(result, Err(GatewayError::NearTransaction(_))));
    }

    /// With nothing to ask, there is no answer.
    #[tokio::test]
    async fn a_configuration_with_no_endpoints_is_an_error_not_an_answer() {
        let mut network =
            NetworkConfig::from_rpc_url("test", "http://127.0.0.1:1".parse().unwrap());
        network.rpc_endpoints.clear();

        let result = query(&NearOperationExecutor::new(network, None)).await;

        assert!(
            matches!(result, Err(GatewayError::NearQuery(_))),
            "an unasked question must not resolve to NoRecord"
        );
    }
}
