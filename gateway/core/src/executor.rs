use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use near_api::{
    advanced::{
        to_final_execution_outcome,
        tx_rpc::{TransactionStatusRef, TransactionStatusRpc},
        ExecuteSignedTransaction, RequestBuilder, ResponseHandler, TransactionableOrSigned,
    },
    errors::{QueryError, RetryError, SendRequestError},
    types::{
        crypto::secret_key::ED25519SecretKey,
        transaction::{
            result::{ExecutionFinalResult, TransactionResult},
            PrepopulateTransaction, SignedTransaction,
        },
        TxExecutionStatus,
    },
    NetworkConfig, SecretKey, Signer,
};
use near_openapi_client::types::{RpcTransactionError, RpcTransactionResponse};
use templar_gateway_types::{operation::ExecutionOutcome, CryptoHash, ManagedAccountId};

use crate::{
    FinalityPolicy, GatewayError, GatewayResult, PlannedTransaction, PooledSigner,
    PreparedTransactionResult, SigningKeyLease,
};

/// Any waiting level makes a node poll a transaction it has never seen until a
/// ~30s timeout, so `UNKNOWN_TRANSACTION` never comes back and a step the chain
/// has no record of can never settle.
const RECONCILIATION_WAIT_UNTIL: TxExecutionStatus = TxExecutionStatus::None;

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
    /// The chain holds the transaction and has not finished it.
    Pending,
}

/// What one set of nodes says, once their answers are reduced.
enum Consensus {
    Executed(StepOutcome),
    Pending,
    /// Every node answered, and none had a record.
    NoRecord,
    /// A node failed to answer, so absence proves nothing.
    Unanswered(GatewayError),
    /// There were no nodes to ask.
    Unasked,
}

/// What a single node says, before the reduction decides what it proves.
enum NodeAnswer {
    Executed(StepOutcome),
    Pending,
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
    /// Retention-complete views of the chain, split the same way. Empty when none
    /// is configured, and then a missing transaction can never be confirmed
    /// missing — see [`TransactionRecord::Unconfirmed`].
    archival_status_networks: Vec<NetworkConfig>,
    /// One single-endpoint view per endpoint, so a status query can be addressed
    /// to each in turn and every node's answer survives.
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
        Self {
            status_query_networks: status_query_networks(&network),
            archival_status_networks: archival_network
                .map_or_else(Vec::new, |network| status_query_networks(&network)),
            network,
            finality_policy,
        }
    }

    /// Ask each node in turn and reduce their answers. A node that holds the
    /// transaction settles the question; absence counts only once every node
    /// agrees, and a node that failed to answer leaves it open.
    async fn poll(
        &self,
        networks: &[NetworkConfig],
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> Consensus {
        let mut unanswered = None;
        let mut answered = false;
        for network in networks {
            match self
                .node_answer_from(network, signer_account_id, tx_hash)
                .await
            {
                Ok(NodeAnswer::Executed(outcome)) => return Consensus::Executed(outcome),
                Ok(NodeAnswer::Pending) => return Consensus::Pending,
                Ok(NodeAnswer::NoRecord) => answered = true,
                Err(error) => unanswered = Some(error),
            }
        }
        match (unanswered, answered) {
            (Some(error), _) => Consensus::Unanswered(error),
            (None, true) => Consensus::NoRecord,
            (None, false) => Consensus::Unasked,
        }
    }

    /// Ask one node what it has, without waiting for the transaction to finish.
    async fn node_answer_from(
        &self,
        network: &NetworkConfig,
        signer_account_id: &ManagedAccountId,
        tx_hash: CryptoHash,
    ) -> GatewayResult<NodeAnswer> {
        match RequestBuilder::new(
            TransactionStatusRpc,
            TransactionStatusRef {
                sender_account_id: signer_account_id.0.clone(),
                tx_hash: tx_hash.0,
                wait_until: RECONCILIATION_WAIT_UNTIL,
            },
            TransactionProgressHandler(self.finality_policy),
        )
        .fetch_from(network)
        .await
        {
            Ok(TransactionResult::Pending { .. }) => Ok(NodeAnswer::Pending),
            Ok(TransactionResult::Full(result)) => {
                Ok(NodeAnswer::Executed(StepOutcome::from_execution(*result)))
            }
            Err(error) if is_unknown_transaction(&error) => Ok(NodeAnswer::NoRecord),
            Err(error) if is_minimal_pending_response(&error) => Ok(NodeAnswer::Pending),
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
        if self.status_query_networks.is_empty() {
            return Err(GatewayError::NearQuery(
                "no RPC endpoint available for transaction status".to_owned(),
            ));
        }

        let unanswered_primary = match self
            .poll(&self.status_query_networks, signer_account_id, tx_hash)
            .await
        {
            Consensus::Executed(outcome) => return Ok(TransactionRecord::Executed(outcome)),
            Consensus::Pending => return Ok(TransactionRecord::Pending),
            Consensus::Unanswered(error) => Some(error),
            Consensus::NoRecord | Consensus::Unasked => None,
        };

        match self
            .poll(&self.archival_status_networks, signer_account_id, tx_hash)
            .await
        {
            Consensus::Executed(outcome) => Ok(TransactionRecord::Executed(outcome)),
            Consensus::Pending => Ok(TransactionRecord::Pending),
            // The only construction of `NoRecord`, reachable only once every
            // retention-complete node agrees — which keeps a primary node's
            // garbage collection from being mistaken for proof that a transaction
            // never executed.
            Consensus::NoRecord => Ok(TransactionRecord::NoRecord),
            Consensus::Unanswered(error) => Err(error),
            Consensus::Unasked => {
                unanswered_primary.map_or(Ok(TransactionRecord::Unconfirmed), Err)
            }
        }
    }
}

/// Applies the completeness bar near_api's own `TransactionStatusHandler` cannot:
/// it discards `final_execution_status`, and the outcome's own `status` is the
/// result of the first leaf receipt, so it resolves while sibling receipts may
/// still be running.
struct TransactionProgressHandler(FinalityPolicy);

impl ResponseHandler for TransactionProgressHandler {
    type Response = TransactionResult;
    type Query = TransactionStatusRpc;

    fn process_response(
        &self,
        responses: Vec<RpcTransactionResponse>,
    ) -> Result<Self::Response, QueryError<RpcTransactionError>> {
        let response = responses
            .into_iter()
            .next()
            .ok_or(QueryError::InternalErrorNoResponse)?;
        let (RpcTransactionResponse::Variant0 {
            final_execution_status,
            ..
        }
        | RpcTransactionResponse::Variant1 {
            final_execution_status,
            ..
        }) = &response;
        let progress = *final_execution_status;
        if !self.0.is_satisfied_by(progress) {
            return Ok(TransactionResult::Pending { status: progress });
        }
        ExecutionFinalResult::try_from(to_final_execution_outcome(response))
            .map(|outcome| TransactionResult::Full(Box::new(outcome)))
            .map_err(|error| QueryError::ConversionError(Box::new(error)))
    }
}

/// The request a failed query actually made it to, if it reached one.
fn failed_request(
    error: &QueryError<RpcTransactionError>,
) -> Option<&SendRequestError<RpcTransactionError>> {
    let QueryError::QueryError(retry) = error else {
        return None;
    };
    match retry.as_ref() {
        RetryError::RetriesExhausted(request) | RetryError::Critical(request) => Some(request),
        _ => None,
    }
}

/// Whether the chain reported having no record of the transaction.
///
/// Matched on the typed error alone. A rendered match would also see the whole
/// response body — progenitor formats it into the transport error — so a body
/// that merely mentions the marker, a contract panic or a log line, would read
/// as the chain's own answer, and past the validity horizon that is terminal.
fn is_unknown_transaction(error: &QueryError<RpcTransactionError>) -> bool {
    matches!(
        failed_request(error),
        Some(SendRequestError::ServerError(
            RpcTransactionError::UnknownTransaction { .. }
        ))
    )
}

/// Whether this error is really the RPC's minimal "not finished yet" answer,
/// which the openapi client cannot deserialize and reports as a transport error.
/// Requiring that exact shape keeps a genuinely malformed payload an error.
fn is_minimal_pending_response(error: &QueryError<RpcTransactionError>) -> bool {
    let Some(SendRequestError::TransportError(near_openapi_client::Error::InvalidResponsePayload(
        body,
        _,
    ))) = failed_request(error)
    else {
        return false;
    };
    serde_json::from_slice::<MinimalTransactionResponse>(body).is_ok_and(|minimal| {
        matches!(
            minimal.result.final_execution_status,
            TxExecutionStatus::None
                | TxExecutionStatus::Included
                | TxExecutionStatus::IncludedFinal
        )
    })
}

#[derive(serde::Deserialize)]
struct MinimalTransactionResponse {
    result: MinimalTransactionResult,
}

#[derive(serde::Deserialize)]
struct MinimalTransactionResult {
    final_execution_status: TxExecutionStatus,
}

/// One single-endpoint view of `network` per endpoint, each tried once.
///
/// Splitting is what keeps every node's answer: near_api walks a multi-endpoint
/// config itself and reports only the last error, which would let one node's "no
/// record" bury another's "still executing". Retries go too — near_api treats
/// `UNKNOWN_TRANSACTION` as retryable and would otherwise sleep through its whole
/// backoff schedule before the answer could be classified.
fn status_query_networks(network: &NetworkConfig) -> Vec<NetworkConfig> {
    network
        .rpc_endpoints
        .iter()
        .map(|endpoint| NetworkConfig {
            rpc_endpoints: vec![endpoint.clone().with_retries(1)],
            ..network.clone()
        })
        .collect()
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
    use rstest::rstest;
    use templar_gateway_types::{CryptoHash, ManagedAccountId};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    use super::{ExecuteOperation, NearOperationExecutor, TransactionRecord};
    use crate::GatewayError;

    /// As a node actually answers for a hash it has no record of — the typed
    /// error carries the requested hash.
    const UNKNOWN_TRANSACTION: &str = r#"{"jsonrpc":"2.0","id":"0","error":{"name":"HANDLER_ERROR","cause":{"name":"UNKNOWN_TRANSACTION","info":{"requested_transaction_hash":"11111111111111111111111111111111"}}}}"#;
    /// A well-formed typed error near_api treats as retryable, so it walks on to
    /// the next endpoint instead of stopping — the case where an answer could be
    /// lost.
    const TIMEOUT_ERROR: &str = r#"{"jsonrpc":"2.0","id":"0","error":{"name":"HANDLER_ERROR","cause":{"name":"TIMEOUT_ERROR"}}}"#;
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

    fn executor_over_many(
        servers: &[&MockServer],
        archival: &[&MockServer],
    ) -> NearOperationExecutor {
        let network_over = |urls: &[&MockServer]| {
            let mut network = NetworkConfig::from_rpc_url("test", urls[0].uri().parse().unwrap());
            network.rpc_endpoints = urls
                .iter()
                .map(|server| RPCEndpoint::new(server.uri().parse().unwrap()))
                .collect();
            network
        };
        NearOperationExecutor::new(
            network_over(servers),
            (!archival.is_empty()).then(|| network_over(archival)),
        )
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

    /// The response a node gives for a transaction it holds but has not finished.
    /// The openapi client cannot deserialize this into a full response, so it
    /// reaches us as a transport error — the route that fires in production.
    const MINIMAL_PENDING: &str =
        r#"{"jsonrpc":"2.0","id":"0","result":{"final_execution_status":"NONE"}}"#;

    /// A real mainnet `tx` response at `wait_until: NONE`, captured verbatim
    /// apart from a shortened action and an emptied receipt list. Its `status` is
    /// `SuccessValue` whatever progress level is substituted, so only
    /// `final_execution_status` separates a finished transaction from one whose
    /// sibling receipts are still running.
    const RESPONSE_TEMPLATE: &str = r#"{"jsonrpc":"2.0","id":"0","result":{"final_execution_status":"{progress}","receipts_outcome":[],"status":{"SuccessValue":""},"transaction":{"actions":[{"Transfer":{"deposit":"1"}}],"hash":"6F3YyM29ajJxENmkyyAYWBQaTtKEJgXjwYVttj74sSSL","nonce":141511583287573,"priority_fee":0,"public_key":"ed25519:GtnhHo73ydoHuRwUykqphBUyPKiZeafdvs1TSg1R8fb6","receiver_id":"coin.abound.near","signature":"ed25519:3FPX3BmPTkfBELwhdxP6dx5kxo1AaxsbVB2yM7RPHsgbBMoZtc8MeiFiFoKp419pcRSjpknkt3Hi2nHwFkfX3XYa","signer_id":"coin.abound.near"},"transaction_outcome":{"block_hash":"HdQKF3DYpN6mwmadHXGDfgpuhGh9XzVyYg5pFBhBGati","id":"6F3YyM29ajJxENmkyyAYWBQaTtKEJgXjwYVttj74sSSL","outcome":{"executor_id":"coin.abound.near","gas_burnt":308231666918,"logs":[],"metadata":{"gas_profile":null,"version":1},"receipt_ids":[],"status":{"SuccessReceiptId":"5D5b4RBt1okZkhXLu7vhFKs7kK2y3g8UBkQYoptmMJo8"},"tokens_burnt":"30823166691800000000"},"proof":[]}}}"#;

    fn full_response(progress: &str) -> String {
        RESPONSE_TEMPLATE.replace("{progress}", progress)
    }

    /// A first-leaf `SuccessValue` is not an outcome until the level the submit
    /// path waits for is reached.
    #[rstest]
    #[case::not_yet_executed("INCLUDED", false)]
    #[case::executed_but_not_final("EXECUTED_OPTIMISTIC", false)]
    #[case::executed("EXECUTED", true)]
    #[case::finalized("FINAL", true)]
    #[tokio::test]
    async fn an_outcome_is_recorded_only_once_it_meets_the_finality_policy(
        #[case] progress: &str,
        #[case] expected_executed: bool,
    ) {
        let primary = responding_with(&full_response(progress)).await;

        let record = query(&executor_over(&[&primary], None)).await.unwrap();

        // The default policy is `Executed`.
        assert_eq!(
            matches!(record, TransactionRecord::Executed(_)),
            expected_executed,
            "{progress} must{} be recorded as an outcome",
            if expected_executed { "" } else { " not" }
        );
    }

    /// A transaction still executing must never be reported as an outcome.
    #[tokio::test]
    async fn a_transaction_still_executing_is_pending_not_an_outcome() {
        let primary = responding_with(MINIMAL_PENDING).await;

        let record = query(&executor_over(&[&primary], None)).await.unwrap();

        assert!(matches!(record, TransactionRecord::Pending));
    }

    /// Holding the transaction settles the question wherever it comes from, so
    /// the walk stops without consulting archival.
    #[tokio::test]
    async fn a_pending_primary_short_circuits_the_archival_query() {
        let primary = responding_with(MINIMAL_PENDING).await;
        let archival = responding_with(UNKNOWN_TRANSACTION).await;

        let record = query(&executor_over(&[&primary], Some(&archival)))
            .await
            .unwrap();

        assert!(matches!(record, TransactionRecord::Pending));
        assert_eq!(
            requests_to(&archival).await,
            0,
            "a pending answer settles it; archival must not be asked"
        );
    }

    /// Tolerating unknown fields is the point: this parser exists to read what
    /// the full one rejected, so refusing a field the full parser did not know
    /// would defeat it — and would silently disable pending detection the next
    /// time a node adds one. Both readings leave the step submitted, so the
    /// tolerance costs no correctness.
    #[tokio::test]
    async fn an_unknown_field_beside_a_pending_status_is_still_pending() {
        let primary = responding_with(
            r#"{"jsonrpc":"2.0","id":"0","result":{"final_execution_status":"NONE","added_by_a_later_node":1}}"#,
        )
        .await;

        let record = query(&executor_over(&[&primary], None)).await.unwrap();

        assert!(matches!(record, TransactionRecord::Pending));
    }

    /// A pending answer from any archival endpoint must survive the walk, in
    /// either order, rather than being lost to a later endpoint's "no record".
    #[rstest]
    #[case::pending_first(&[MINIMAL_PENDING, UNKNOWN_TRANSACTION])]
    #[case::unknown_first(&[UNKNOWN_TRANSACTION, MINIMAL_PENDING])]
    #[tokio::test]
    async fn an_archival_pending_answer_outranks_another_endpoints_no_record(
        #[case] archival_bodies: &[&str],
    ) {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;
        let mut archival = Vec::new();
        for body in archival_bodies {
            archival.push(responding_with(body).await);
        }
        let archival_refs: Vec<&MockServer> = archival.iter().collect();

        let record = query(&executor_over_many(&[&primary], &archival_refs))
            .await
            .unwrap();

        assert!(
            matches!(record, TransactionRecord::Pending),
            "an archival node holding the transaction must not be overruled"
        );
    }

    /// An archival node that failed to answer keeps the question open, even when
    /// another reports no record: absence is only evidence once every
    /// retention-complete node agrees.
    #[tokio::test]
    async fn one_silent_archival_endpoint_blocks_a_no_record_verdict() {
        let primary = responding_with(UNKNOWN_TRANSACTION).await;
        let unreachable = responding_with(TIMEOUT_ERROR).await;
        let archival = responding_with(UNKNOWN_TRANSACTION).await;

        let result = query(&executor_over_many(&[&primary], &[&unreachable, &archival])).await;

        assert!(
            matches!(result, Err(GatewayError::NearTransaction(_))),
            "an archival endpoint that did not answer must not resolve to NoRecord"
        );
    }

    /// A malformed body is not a pending answer.
    #[tokio::test]
    async fn an_undeserializable_response_is_an_error_not_pending() {
        let primary =
            responding_with(r#"{"jsonrpc":"2.0","id":"0","result":{"nonsense":1}}"#).await;

        let result = query(&executor_over(&[&primary], None)).await;

        assert!(
            matches!(result, Err(GatewayError::NearTransaction(_))),
            "only the minimal status shape may be read as pending"
        );
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
