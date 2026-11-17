//! Raw sandbox RPC and launch configuration shared by tests and tools.

use std::time::Duration;

use anyhow::{Context, Result};
use near_api::{types::AccountId, NetworkConfig};
use near_jsonrpc_client::{
    errors::{
        JsonRpcError, JsonRpcServerError, JsonRpcServerResponseStatusError,
        JsonRpcTransportRecvError, JsonRpcTransportSendError, RpcTransportError,
    },
    methods::{
        query::RpcQueryRequest, sandbox_fast_forward::RpcSandboxFastForwardRequest,
        sandbox_patch_state::RpcSandboxPatchStateRequest, status::RpcStatusRequest,
    },
    JsonRpcClient,
};
use near_primitives::{
    state_record::StateRecord,
    types::{BlockReference, Finality, StoreKey, StoreValue},
    views::{AccountView, QueryRequest},
};
use near_sandbox::{
    config::{DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY, DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY},
    GenesisAccount, SandboxConfig,
};
use near_token::NearToken;
use serde::Serialize;

const RPC_TIMEOUT: Duration = Duration::from_secs(120);
const FINALITY_TIMEOUT: Duration = Duration::from_secs(60);
const FINALITY_POLL_MIN: Duration = Duration::from_millis(25);
const FINALITY_POLL_MAX: Duration = Duration::from_millis(500);
const STOCK_MIN_BLOCK_MS: u64 = 120;
const STOCK_MAX_BLOCK_MS: u64 = 500;
const FAST_FORWARD_BLOCK_MS: u64 = (STOCK_MIN_BLOCK_MS + STOCK_MAX_BLOCK_MS) / 2;
const MIN_BLOCK_MS: u64 = 40;

/// The high-balance genesis account used by sandbox harnesses. It reuses the
/// default genesis keypair because shared test runs exhaust `sandbox`.
pub const FUNDER_ACCOUNT_ID: &str = "funder";
fn build_client(rpc_url: &str, timeout: Duration) -> JsonRpcClient {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .default_headers(headers)
        .build()
        .unwrap_or_else(|error| panic!("reqwest client builds: {error}"));
    JsonRpcClient::with(http).connect(rpc_url)
}

fn client(network: &NetworkConfig) -> JsonRpcClient {
    build_client(network.rpc_endpoints[0].url.as_str(), RPC_TIMEOUT)
}

/// Whether the sandbox node at `rpc_url` answers a status query within `timeout`.
pub async fn node_is_serving(rpc_url: &str, timeout: Duration) -> bool {
    build_client(rpc_url, timeout)
        .call(RpcStatusRequest)
        .await
        .is_ok()
}

/// Patch raw contract storage entries on `account_id`.
pub async fn patch_data(
    network: &NetworkConfig,
    account_id: &AccountId,
    entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    let records = entries
        .into_iter()
        .map(|(key, value)| StateRecord::Data {
            account_id: account_id.clone(),
            data_key: StoreKey::from(key),
            value: StoreValue::from(value),
        })
        .collect();
    patch_records(network, records).await
}

/// State patches commit before the RPC returns on the pinned sandbox release, but still need to
/// reach `Final` before signing with a patched key, because near-api reads signer keys at `Final`.
///
/// Large patches go over several requests, applied in order but not atomically: a failure leaves
/// earlier batches applied, and only a record in the last batch reaching `Final` implies the rest
/// has.
pub async fn patch_records(network: &NetworkConfig, records: Vec<StateRecord>) -> Result<()> {
    let client = client(network);
    let batches = batches(records)?;
    let count = batches.len();
    for (index, records) in batches.into_iter().enumerate() {
        let len = records.len();
        client
            .call(RpcSandboxPatchStateRequest { records })
            .await
            .with_context(|| {
                format!(
                    "sandbox_patch_state failed on batch {} of {count} ({len} records); earlier \
                     batches stay applied",
                    index + 1
                )
            })?;
    }
    Ok(())
}

/// neard's RPC refuses a request body over 2 MiB whatever `json_payload_max_size` allows, so one
/// record larger than this cannot be patched at all.
const PATCH_REQUEST_BYTES: usize = 1536 * 1024;

/// Split `records` into requests of at most [`PATCH_REQUEST_BYTES`] each, in order; a single
/// larger record travels alone.
fn batches(records: Vec<StateRecord>) -> Result<Vec<Vec<StateRecord>>> {
    let mut batches: Vec<Vec<StateRecord>> = Vec::new();
    let mut size = 0;
    for record in records {
        let record_size = serde_json::to_vec(&record)?.len();
        match batches.last_mut() {
            Some(batch) if size + record_size <= PATCH_REQUEST_BYTES => batch.push(record),
            _ => {
                batches.push(vec![record]);
                size = 0;
            }
        }
        size += record_size;
    }
    Ok(batches)
}

/// Advance the sandbox chain by `delta_height` blocks.
pub async fn fast_forward(network: &NetworkConfig, delta_height: u64) -> Result<()> {
    client(network)
        .call(RpcSandboxFastForwardRequest { delta_height })
        .await
        .context("sandbox_fast_forward failed")?;
    Ok(())
}

/// Wait for a patched key to reach `Final` before using it to sign, with bounded
/// backoff to avoid amplifying an overloaded sandbox node.
pub async fn wait_until_final(
    network: &NetworkConfig,
    account_id: &AccountId,
    public_key: &near_crypto::PublicKey,
) -> Result<()> {
    let client = client(network);
    let request = RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::ViewAccessKey {
            account_id: account_id.clone(),
            public_key: public_key.clone(),
        },
    };
    tokio::time::timeout(FINALITY_TIMEOUT, async {
        let mut backoff = FINALITY_POLL_MIN;
        while client.call(&request).await.is_err() {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(FINALITY_POLL_MAX);
        }
    })
    .await
    .with_context(|| {
        format!(
            "patched account {account_id} never reached final finality within \
             {FINALITY_TIMEOUT:?} — the sandbox node is likely overloaded or down"
        )
    })
}

/// Wait for an account metadata patch to be visible in finalized state.
///
/// `sandbox_patch_state` waits for committed state on the pinned sandbox release, not finalized
/// visibility. An already-present access key cannot fence a later metadata patch, so compare the
/// exact balance, storage usage and code linkage.
///
/// Retry transient RPC failures within the same deadline, but reject invalid requests,
/// malformed responses and account/domain errors immediately.
pub async fn wait_for_account_final(
    network: &NetworkConfig,
    account_id: &AccountId,
    expected: &AccountView,
) -> Result<()> {
    let client = client(network);
    let request = RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::ViewAccount {
            account_id: account_id.clone(),
        },
    };
    let mut last_seen = None;
    let mut last_retryable_error = None;
    tokio::time::timeout(FINALITY_TIMEOUT, async {
        let mut backoff = FINALITY_POLL_MIN;
        loop {
            match client.call(&request).await {
                Ok(response) => {
                    let near_jsonrpc_primitives::types::query::QueryResponseKind::ViewAccount(
                        actual,
                    ) = response.kind
                    else {
                        anyhow::bail!("unexpected finalized account response for {account_id}");
                    };
                    if &actual == expected {
                        return Ok(());
                    }
                    last_seen = Some((response.block_hash, actual));
                }
                Err(error) if retryable_account_query_error(&error) => {
                    last_retryable_error = Some(error);
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("read finalized sandbox account {account_id}"));
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(FINALITY_POLL_MAX);
        }
    })
    .await
    .with_context(|| {
        format!(
            "sandbox account {account_id} metadata did not reach finality within \
             {FINALITY_TIMEOUT:?}; expected {expected:?}; last finalized observation {last_seen:?}; \
             last retryable RPC error {last_retryable_error:?}"
        )
    })?
}

fn retryable_account_query_error(
    error: &JsonRpcError<near_jsonrpc_primitives::types::query::RpcQueryError>,
) -> bool {
    use near_jsonrpc_primitives::types::query::RpcQueryError;

    match error {
        JsonRpcError::TransportError(
            RpcTransportError::SendError(JsonRpcTransportSendError::PayloadSendError(error))
            | RpcTransportError::RecvError(JsonRpcTransportRecvError::PayloadRecvError(error)),
        ) => error.is_timeout() || error.is_connect() || error.is_body(),
        JsonRpcError::ServerError(
            JsonRpcServerError::InternalError { .. }
            | JsonRpcServerError::HandlerError(
                RpcQueryError::NoSyncedBlocks | RpcQueryError::InternalError { .. },
            )
            | JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::TooManyRequests
                | JsonRpcServerResponseStatusError::TimeoutError
                | JsonRpcServerResponseStatusError::ServiceUnavailable,
            ),
        ) => true,
        JsonRpcError::ServerError(JsonRpcServerError::ResponseStatusError(
            JsonRpcServerResponseStatusError::Unexpected { status },
        )) => matches!(
            *status,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
                | reqwest::StatusCode::BAD_GATEWAY
                | reqwest::StatusCode::GATEWAY_TIMEOUT
        ),
        _ => false,
    }
}

/// Sandbox launch configuration shared by owned and out-of-band nodes.
#[must_use]
pub fn sandbox_config() -> SandboxConfig {
    let (min_block_ms, max_block_ms) = block_delays_ms();
    SandboxConfig {
        additional_config: Some(
            serde_json::to_value(AdditionalConfig {
                consensus: ConsensusConfig {
                    min_block_production_delay: duration_json(min_block_ms),
                    max_block_production_delay: duration_json(max_block_ms),
                },
                trie_viewer_state_size_limit: TRIE_VIEWER_STATE_SIZE_LIMIT,
            })
            .unwrap_or_else(|error| panic!("sandbox config serializes: {error}")),
        ),
        additional_accounts: vec![GenesisAccount {
            account_id: FUNDER_ACCOUNT_ID
                .parse()
                .unwrap_or_else(|error| panic!("funder account id is valid: {error}")),
            public_key: DEFAULT_GENESIS_ACCOUNT_PUBLIC_KEY.to_string(),
            private_key: DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY.to_string(),
            balance: NearToken::from_near(100_000_000),
        }],
        ..SandboxConfig::default()
    }
}

/// The default 50 kB refuses `view_state` for any account holding a contract blob.
const TRIE_VIEWER_STATE_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Serialize)]
struct AdditionalConfig {
    consensus: ConsensusConfig,
    trie_viewer_state_size_limit: u64,
}

#[derive(Serialize)]
struct ConsensusConfig {
    min_block_production_delay: DurationJson,
    max_block_production_delay: DurationJson,
}

#[derive(Serialize)]
struct DurationJson {
    secs: u64,
    nanos: u64,
}

fn duration_json(ms: u64) -> DurationJson {
    DurationJson {
        secs: ms / 1_000,
        nanos: (ms % 1_000) * 1_000_000,
    }
}

fn block_delays_ms() -> (u64, u64) {
    let min = match std::env::var("NEAR_SANDBOX_BLOCK_MS") {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .unwrap_or_else(|_| {
                panic!(
                    "NEAR_SANDBOX_BLOCK_MS must be a whole number of milliseconds, got `{value}`"
                )
            })
            .clamp(1, FAST_FORWARD_BLOCK_MS),
        Err(_) => MIN_BLOCK_MS,
    };
    (min, 2 * FAST_FORWARD_BLOCK_MS - min)
}

#[cfg(test)]
mod tests {
    use near_primitives::{
        state_record::StateRecord,
        types::{StoreKey, StoreValue},
    };

    use super::{batches, PATCH_REQUEST_BYTES};

    fn record(index: u8, len: usize) -> StateRecord {
        StateRecord::Data {
            account_id: "registry.near".parse().unwrap(),
            data_key: StoreKey::from(vec![index]),
            value: StoreValue::from(vec![index; len]),
        }
    }

    fn index_of(record: &StateRecord) -> u8 {
        match record {
            StateRecord::Data { data_key, .. } => data_key[0],
            _ => unreachable!(),
        }
    }

    #[test]
    fn batches_keep_order_stay_bounded_and_send_an_oversized_record_alone() {
        let records = vec![
            record(0, 100),
            record(1, PATCH_REQUEST_BYTES),
            record(2, PATCH_REQUEST_BYTES / 3),
            record(3, PATCH_REQUEST_BYTES / 3),
            record(4, PATCH_REQUEST_BYTES / 2),
        ];

        let batches = batches(records).unwrap();

        let order: Vec<Vec<u8>> = batches
            .iter()
            .map(|batch| batch.iter().map(index_of).collect())
            .collect();
        assert_eq!(order, vec![vec![0], vec![1], vec![2, 3], vec![4]]);
        for batch in batches.iter().filter(|batch| batch.len() > 1) {
            let size: usize = batch
                .iter()
                .map(|record| serde_json::to_vec(record).unwrap().len())
                .sum();
            assert!(size <= PATCH_REQUEST_BYTES, "{size}");
        }
    }

    #[test]
    fn no_records_means_no_requests() {
        assert!(batches(Vec::new()).unwrap().is_empty());
    }
}

#[cfg(test)]
mod account_finality_tests {
    use std::{collections::VecDeque, sync::Arc};

    use axum::{
        extract::State,
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use near_jsonrpc_primitives::{
        errors::RpcError,
        types::query::{QueryResponseKind, RpcQueryError, RpcQueryResponse},
    };
    use near_primitives::{hash::CryptoHash, views::AccountView};
    use serde_json::{json, Value};
    use tokio::{
        sync::{mpsc, Mutex},
        task::JoinHandle,
    };

    use super::{
        retryable_account_query_error, wait_for_account_final, JsonRpcError, JsonRpcServerError,
        JsonRpcServerResponseStatusError, NetworkConfig, FINALITY_TIMEOUT,
    };

    enum Reply {
        Result(Value),
        Error(RpcError),
        Status(StatusCode),
        Malformed,
        Pending,
    }

    #[derive(Clone)]
    struct ServerState {
        replies: Arc<Mutex<VecDeque<Reply>>>,
        requests: mpsc::UnboundedSender<Value>,
    }

    async fn respond(State(state): State<ServerState>, Json(request): Json<Value>) -> Response {
        let reply = state.replies.lock().await.pop_front().unwrap();
        state.requests.send(request.clone()).unwrap();
        match reply {
            Reply::Result(result) => Json(json!({
                "jsonrpc": "2.0", "id": request["id"], "result": result,
            }))
            .into_response(),
            Reply::Error(error) => Json(json!({
                "jsonrpc": "2.0", "id": request["id"], "error": error,
            }))
            .into_response(),
            Reply::Status(status) => status.into_response(),
            Reply::Malformed => (StatusCode::OK, "{not json").into_response(),
            Reply::Pending => std::future::pending().await,
        }
    }

    struct RpcFixture {
        network: NetworkConfig,
        requests: mpsc::UnboundedReceiver<Value>,
        server: JoinHandle<()>,
    }

    impl RpcFixture {
        async fn new(replies: Vec<Reply>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (requests, receiver) = mpsc::unbounded_channel();
            let app = Router::new()
                .route("/", post(respond))
                .with_state(ServerState {
                    replies: Arc::new(Mutex::new(replies.into())),
                    requests,
                });
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                network: NetworkConfig::from_rpc_url("account-finality-test", url.parse().unwrap()),
                requests: receiver,
                server,
            }
        }

        async fn assert_final_account_request(&mut self) {
            let request = self.requests.recv().await.unwrap();
            assert_eq!(request["method"], "query");
            assert_eq!(
                request["params"],
                json!({
                    "request_type": "view_account",
                    "account_id": "market.near",
                    "finality": "final",
                })
            );
        }
    }

    impl Drop for RpcFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn account() -> AccountView {
        serde_json::from_value(json!({
            "amount": "1000",
            "locked": "0",
            "code_hash": CryptoHash::default(),
            "storage_usage": 100,
        }))
        .unwrap()
    }

    fn observed(account: AccountView) -> Reply {
        Reply::Result(
            serde_json::to_value(RpcQueryResponse {
                kind: QueryResponseKind::ViewAccount(account),
                block_height: 42,
                block_hash: CryptoHash([7; 32]),
            })
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn account_finality_recovers_from_transient_errors_and_compares_exact_metadata() {
        let expected = account();
        let mut stale = expected.clone();
        stale.global_contract_account_id = Some("old-code.near".parse().unwrap());
        let mut fixture = RpcFixture::new(vec![
            Reply::Status(StatusCode::SERVICE_UNAVAILABLE),
            Reply::Error(
                RpcQueryError::InternalError {
                    error_message: "node overloaded".into(),
                }
                .into(),
            ),
            observed(stale),
            observed(expected.clone()),
        ])
        .await;

        wait_for_account_final(&fixture.network, &"market.near".parse().unwrap(), &expected)
            .await
            .unwrap();
        for _ in 0..4 {
            fixture.assert_final_account_request().await;
        }
        assert!(fixture.requests.try_recv().is_err());
    }

    #[test]
    fn account_finality_retries_only_transient_server_statuses() {
        for (status, retryable) in [
            (StatusCode::INTERNAL_SERVER_ERROR, true),
            (StatusCode::BAD_GATEWAY, true),
            (StatusCode::GATEWAY_TIMEOUT, true),
            (StatusCode::NOT_IMPLEMENTED, false),
            (StatusCode::HTTP_VERSION_NOT_SUPPORTED, false),
            (StatusCode::FORBIDDEN, false),
            (StatusCode::NOT_FOUND, false),
        ] {
            let error = JsonRpcError::ServerError(JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::Unexpected { status },
            ));
            assert_eq!(retryable_account_query_error(&error), retryable, "{status}");
        }
        for status in [
            JsonRpcServerResponseStatusError::TooManyRequests,
            JsonRpcServerResponseStatusError::TimeoutError,
            JsonRpcServerResponseStatusError::ServiceUnavailable,
        ] {
            let error = JsonRpcError::ServerError(JsonRpcServerError::ResponseStatusError(status));
            assert!(retryable_account_query_error(&error));
        }
    }

    #[tokio::test]
    async fn account_finality_rejects_permanent_errors_without_retrying() {
        let expected = account();
        let failures = [
            Reply::Status(StatusCode::UNAUTHORIZED),
            Reply::Status(StatusCode::BAD_REQUEST),
            Reply::Status(StatusCode::FORBIDDEN),
            Reply::Status(StatusCode::NOT_IMPLEMENTED),
            Reply::Malformed,
            Reply::Result(json!({"unexpected": "query result"})),
            Reply::Result(json!({
                "values": [], "proof": [], "block_height": 42,
                "block_hash": CryptoHash::default(),
            })),
            Reply::Error(
                RpcQueryError::UnknownAccount {
                    requested_account_id: "market.near".parse().unwrap(),
                    block_height: 42,
                    block_hash: CryptoHash::default(),
                }
                .into(),
            ),
            Reply::Error(RpcError::parse_error("invalid request parameters".into())),
            Reply::Error(RpcError::new(-32_602, "Invalid params".into(), None)),
        ];
        for failure in failures {
            let mut fixture = RpcFixture::new(vec![failure, observed(expected.clone())]).await;
            let error = wait_for_account_final(
                &fixture.network,
                &"market.near".parse().unwrap(),
                &expected,
            )
            .await
            .unwrap_err();
            assert!(error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_none());
            fixture.assert_final_account_request().await;
            assert!(fixture.requests.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn account_finality_timeout_retains_observation_and_last_transient_error() {
        let expected = account();
        let mut stale = expected.clone();
        stale.storage_usage = 99;
        let mut fixture = RpcFixture::new(vec![
            observed(stale.clone()),
            Reply::Status(StatusCode::SERVICE_UNAVAILABLE),
            Reply::Pending,
        ])
        .await;
        let network = fixture.network.clone();
        let expected_for_task = expected.clone();
        let task = tokio::spawn(async move {
            wait_for_account_final(
                &network,
                &"market.near".parse().unwrap(),
                &expected_for_task,
            )
            .await
        });
        // The third request proves the helper consumed both preceding responses. Freeze time
        // only now, so the HTTP exchanges cannot race Tokio's idle-clock auto-advancement.
        for _ in 0..3 {
            fixture.assert_final_account_request().await;
        }
        tokio::time::pause();
        tokio::time::advance(FINALITY_TIMEOUT).await;
        let error = task.await.unwrap().unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(error
            .downcast_ref::<tokio::time::error::Elapsed>()
            .is_some());
        assert!(
            diagnostic.contains(&stale.storage_usage.to_string()),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&CryptoHash([7; 32]).to_string()),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("ServiceUnavailable"), "{diagnostic}");
        assert!(fixture.requests.try_recv().is_err());
    }
}
