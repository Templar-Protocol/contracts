//! The gateway's only path to a NEAR RPC endpoint.
//!
//! Every request goes through [`NearRpc`], so a refused API key (401, 402 or
//! 403) fails over to the backup key on every read, signature and submission
//! alike. `gateway/clippy.toml` rejects near-api's network-taking methods
//! anywhere else.

use std::{
    fmt,
    future::Future,
    iter,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use near_api::{
    advanced::{ExecuteSignedTransaction, ResponseHandler, RpcBuilder, RpcType},
    errors::{ExecuteTransactionError, QueryError, RetryError, SendRequestError, SignerError},
    types::{transaction::result::TransactionResult, BlockHeight, CryptoHash, Nonce},
    AccountId, NetworkConfig, PublicKey, Signer,
};

/// How long a refused primary key is skipped before it is tried again, matching
/// the Go services' key failover.
const PRIMARY_REFUSAL_PAUSE: Duration = Duration::from_secs(60);

const RAW_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A NEAR network reached with a primary API key and, optionally, a backup key
/// on the same endpoints.
///
/// A call that the primary key's endpoint refuses (401, 402 or 403: the key or
/// its account, not the request) is repeated once with the backup key, and the
/// primary is then skipped for a minute. Other failures are returned as they
/// are. Clones share that pause, so one refusal moves reads, signing and
/// submission together.
#[derive(Clone)]
pub struct NearRpc {
    primary: Arc<NetworkConfig>,
    /// `primary` with only the key replaced, endpoint for endpoint.
    backup: Option<Arc<NetworkConfig>>,
    pause: Arc<PrimaryPause>,
}

impl From<NetworkConfig> for NearRpc {
    fn from(network: NetworkConfig) -> Self {
        Self::from_parts(network, None)
    }
}

impl NearRpc {
    /// `network` as the primary, with a backup that sends `backup_api_key`
    /// wherever the primary sends a key. Without a primary key there is no
    /// key to refuse, so no backup either.
    #[must_use]
    pub fn with_backup_api_key(network: NetworkConfig, backup_api_key: &str) -> Self {
        let backup = network
            .rpc_endpoints
            .iter()
            .any(|endpoint| endpoint.bearer_header.is_some())
            .then(|| NetworkConfig {
                rpc_endpoints: network
                    .rpc_endpoints
                    .iter()
                    .cloned()
                    .map(|endpoint| match endpoint.bearer_header {
                        Some(_) => endpoint.with_api_key(backup_api_key.to_owned()),
                        None => endpoint,
                    })
                    .collect(),
                ..network.clone()
            });
        Self::from_parts(network, backup)
    }

    fn from_parts(primary: NetworkConfig, backup: Option<NetworkConfig>) -> Self {
        Self {
            primary: Arc::new(at_least_one_attempt(primary)),
            backup: backup.map(|backup| Arc::new(at_least_one_attempt(backup))),
            pause: Arc::new(PrimaryPause::new(PRIMARY_REFUSAL_PAUSE)),
        }
    }

    /// The URLs requests are sent to.
    pub fn endpoint_urls(&self) -> impl Iterator<Item = &url::Url> {
        self.primary
            .rpc_endpoints
            .iter()
            .map(|endpoint| &endpoint.url)
    }

    /// Whether any API key would travel over a non-`https` endpoint.
    #[must_use]
    pub fn sends_key_in_cleartext(&self) -> bool {
        iter::once(&*self.primary)
            .chain(self.backup.as_deref())
            .flat_map(|network| &network.rpc_endpoints)
            .any(|endpoint| endpoint.bearer_header.is_some() && endpoint.url.scheme() != "https")
    }

    /// One single-endpoint view per endpoint, sharing this network's keys and
    /// pause. Each is tried once: near-api retries `UNKNOWN_TRANSACTION`, so it
    /// would sleep through its whole backoff before that answer is seen.
    pub(crate) fn per_endpoint(&self) -> Vec<Self> {
        (0..self.primary.rpc_endpoints.len())
            .map(|index| Self {
                primary: Arc::new(single_endpoint(&self.primary, index)),
                backup: self
                    .backup
                    .as_deref()
                    .map(|backup| Arc::new(single_endpoint(backup, index))),
                pause: Arc::clone(&self.pause),
            })
            .collect()
    }

    /// Send a near-api query. `request` builds it afresh for each key tried.
    #[expect(
        clippy::disallowed_methods,
        reason = "NearRpc is the one place allowed to reach the network"
    )]
    pub(crate) async fn fetch<Query, Handler>(
        &self,
        request: impl Fn() -> RpcBuilder<Query, Handler> + Send + Sync,
    ) -> Result<Handler::Response, QueryError<Query::Error>>
    where
        Handler: ResponseHandler<Query = Query> + Send + Sync,
        Query: RpcType + 'static,
        Query::Response: fmt::Debug + Send + Sync,
        Query::Error: fmt::Debug + Send + Sync,
        Handler::Response: Send,
    {
        self.with_failover(|network| {
            let request = request();
            async move { request.fetch_from(&network).await }
        })
        .await
    }

    /// Broadcast a transaction. Resending it with the backup key is safe: a
    /// refused request never reached a node, and a resent transaction is
    /// recognized by its hash.
    #[expect(
        clippy::disallowed_methods,
        reason = "NearRpc is the one place allowed to reach the network"
    )]
    pub(crate) async fn send(
        &self,
        transaction: impl Fn() -> ExecuteSignedTransaction + Send + Sync,
    ) -> Result<TransactionResult, ExecuteTransactionError> {
        self.with_failover(|network| {
            let transaction = transaction();
            async move { transaction.send_to(&network).await }
        })
        .await
    }

    /// Allocate `public_key`'s next nonce from its on-chain access key.
    #[expect(
        clippy::disallowed_methods,
        reason = "NearRpc is the one place allowed to reach the network"
    )]
    pub(crate) async fn fetch_tx_nonce(
        &self,
        signer: &Signer,
        account_id: &AccountId,
        public_key: PublicKey,
    ) -> Result<(Nonce, CryptoHash, BlockHeight), SignerError> {
        self.with_failover(|network| {
            let account_id = account_id.clone();
            async move {
                signer
                    .fetch_tx_nonce(account_id, public_key, &network)
                    .await
            }
        })
        .await
    }

    /// Send a JSON-RPC request near-api has no builder for.
    pub(crate) async fn raw<R: RawRpcRequest + Sync>(
        &self,
        request: &R,
    ) -> Result<R::Output, RawRpcError> {
        self.with_failover(|network| async move { raw_request(&network, request).await })
            .await
    }

    async fn with_failover<T, E, F, Fut>(&self, attempt: F) -> Result<T, E>
    where
        E: RefusedKey,
        F: Fn(Arc<NetworkConfig>) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, E>> + Send,
    {
        let Some(backup) = &self.backup else {
            return attempt(Arc::clone(&self.primary)).await;
        };

        if self.pause.is_active() {
            let result = attempt(Arc::clone(backup)).await;
            if refused_status(&result).is_none() {
                return result;
            }
            let result = attempt(Arc::clone(&self.primary)).await;
            if refused_status(&result).is_none() {
                self.pause.clear();
            }
            return result;
        }

        let result = attempt(Arc::clone(&self.primary)).await;
        let Some(status) = refused_status(&result) else {
            return result;
        };
        let newly_paused = self.pause.start();
        if newly_paused {
            tracing::warn!(status, "API key refused; sending the backup key");
        }
        let result = attempt(Arc::clone(backup)).await;
        if let (true, Some(status)) = (newly_paused, refused_status(&result)) {
            tracing::warn!(status, "backup API key refused too");
        }
        result
    }
}

/// Endpoint URLs and whether a backup key is set; never a key, a path or a
/// query, any of which may carry one.
impl fmt::Debug for NearRpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let endpoints: Vec<_> = self
            .endpoint_urls()
            .map(|url| url.origin().ascii_serialization())
            .collect();
        f.debug_struct("NearRpc")
            .field("endpoints", &endpoints)
            .field("backup_key", &self.backup.is_some())
            .finish_non_exhaustive()
    }
}

/// near-api's retry loop panics on an endpoint allowed zero attempts.
fn at_least_one_attempt(mut network: NetworkConfig) -> NetworkConfig {
    for endpoint in &mut network.rpc_endpoints {
        endpoint.retries = endpoint.retries.max(1);
    }
    network
}

fn single_endpoint(network: &NetworkConfig, index: usize) -> NetworkConfig {
    NetworkConfig {
        rpc_endpoints: network
            .rpc_endpoints
            .get(index)
            .cloned()
            .map(|endpoint| endpoint.with_retries(1))
            .into_iter()
            .collect(),
        ..network.clone()
    }
}

#[derive(Debug)]
struct PrimaryPause {
    duration: Duration,
    until: Mutex<Option<Instant>>,
}

impl PrimaryPause {
    fn new(duration: Duration) -> Self {
        Self {
            duration,
            until: Mutex::new(None),
        }
    }

    fn is_active(&self) -> bool {
        self.until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|until| Instant::now() < until)
    }

    /// Pause from now. True when no pause was already running, so a refusal is
    /// reported once per pause rather than once per call.
    fn start(&self) -> bool {
        let now = Instant::now();
        let mut until = self.until.lock().unwrap_or_else(PoisonError::into_inner);
        let was_active = until.is_some_and(|until| now < until);
        *until = Some(now + self.duration);
        !was_active
    }

    fn clear(&self) {
        *self.until.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

const fn is_refusal(status: u16) -> bool {
    matches!(status, 401..=403)
}

fn refused_status<T, E: RefusedKey>(result: &Result<T, E>) -> Option<u16> {
    result.as_ref().err().and_then(RefusedKey::refused_status)
}

/// The HTTP status of a failure caused by the API key rather than the request.
pub(crate) trait RefusedKey {
    fn refused_status(&self) -> Option<u16>;
}

impl<E: fmt::Debug + Send + Sync> RefusedKey for SendRequestError<E> {
    fn refused_status(&self) -> Option<u16> {
        let Self::TransportError(error) = self else {
            return None;
        };
        error
            .status()
            .map(|status| status.as_u16())
            .filter(|status| is_refusal(*status))
    }
}

impl<E: RefusedKey> RefusedKey for RetryError<E> {
    fn refused_status(&self) -> Option<u16> {
        retried_request(self).and_then(RefusedKey::refused_status)
    }
}

impl<E: fmt::Debug + Send + Sync> RefusedKey for QueryError<E> {
    fn refused_status(&self) -> Option<u16> {
        failed_request(self).and_then(RefusedKey::refused_status)
    }
}

impl RefusedKey for SignerError {
    fn refused_status(&self) -> Option<u16> {
        match self {
            Self::FetchNonceError(error) => error.refused_status(),
            _ => None,
        }
    }
}

impl RefusedKey for ExecuteTransactionError {
    fn refused_status(&self) -> Option<u16> {
        match self {
            Self::PreQueryError(error) => error.refused_status(),
            Self::SignerError(error) => error.refused_status(),
            Self::TransactionError(error) => error.refused_status(),
            _ => None,
        }
    }
}

/// The request a failed query actually made it to, if it reached one.
pub(crate) fn failed_request<E: fmt::Debug + Send + Sync>(
    error: &QueryError<E>,
) -> Option<&SendRequestError<E>> {
    let QueryError::QueryError(retry) = error else {
        return None;
    };
    retried_request(retry)
}

fn retried_request<E>(error: &RetryError<E>) -> Option<&E> {
    match error {
        RetryError::RetriesExhausted(request) | RetryError::Critical(request) => Some(request),
        RetryError::NoRpcEndpoints | RetryError::InvalidApiKey(_) => None,
    }
}

/// A JSON-RPC request sent without near-api. A JSON-RPC error moves on to the
/// next endpoint at once; only transport failures are retried.
pub(crate) trait RawRpcRequest {
    type Output: Send;

    fn query(
        &self,
        client: &near_openapi_client::Client,
    ) -> impl Future<Output = Result<Result<Self::Output, String>, near_openapi_client::Error<()>>> + Send;
}

#[derive(Debug)]
pub(crate) struct RawRpcError {
    message: String,
    refused_status: Option<u16>,
}

impl RawRpcError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            refused_status: None,
        }
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }
}

impl RefusedKey for RawRpcError {
    fn refused_status(&self) -> Option<u16> {
        self.refused_status
    }
}

async fn raw_request<R: RawRpcRequest + Sync>(
    network: &NetworkConfig,
    request: &R,
) -> Result<R::Output, RawRpcError> {
    let mut last_error = RawRpcError::new("no RPC endpoints are configured");
    // A later endpoint's unrelated failure must not hide that the key was refused.
    let mut refused = None;

    'endpoint: for endpoint in &network.rpc_endpoints {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(value) = &endpoint.bearer_header {
            let mut header = match reqwest::header::HeaderValue::from_str(value) {
                Ok(header) => header,
                Err(error) => {
                    last_error = RawRpcError::new(error.to_string());
                    continue 'endpoint;
                }
            };
            header.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, header.clone());
            headers.insert(
                reqwest::header::HeaderName::from_static("x-api-key"),
                header,
            );
        }
        let http = reqwest::ClientBuilder::new()
            .connect_timeout(RAW_REQUEST_TIMEOUT)
            .timeout(RAW_REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .map_err(|error| RawRpcError::new(error.to_string()))?;
        let client = near_openapi_client::Client::new_with_client(
            endpoint.url.as_str().trim_end_matches('/'),
            http,
        );

        let attempts = usize::from(endpoint.retries).max(1);
        for attempt in 0..attempts {
            match request.query(&client).await {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(error)) => {
                    last_error = RawRpcError::new(error);
                    continue 'endpoint;
                }
                Err(error) => {
                    let refused_status = error
                        .status()
                        .map(|status| status.as_u16())
                        .filter(|status| is_refusal(*status));
                    last_error = RawRpcError {
                        message: error.to_string(),
                        refused_status,
                    };
                    if refused_status.is_some() {
                        refused = refused_status;
                        continue 'endpoint;
                    }
                    if attempt + 1 < attempts {
                        tokio::time::sleep(endpoint.get_sleep_duration(attempt)).await;
                    }
                }
            }
        }
    }

    last_error.refused_status = last_error.refused_status.or(refused);
    Err(last_error)
}

/// Mock endpoints that answer each API key differently.
#[cfg(test)]
pub(crate) mod keyed_endpoint {
    use near_api::{NetworkConfig, RPCEndpoint};
    use wiremock::{
        matchers::{header, method},
        Mock, MockServer, ResponseTemplate,
    };

    use super::NearRpc;

    pub(crate) const PRIMARY: &str = "PRIMARY-SECRET";
    pub(crate) const BACKUP: &str = "BACKUP-SECRET";

    pub(crate) async fn serving(primary: ResponseTemplate, backup: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        for (key, response) in [(PRIMARY, primary), (BACKUP, backup)] {
            Mock::given(method("POST"))
                .and(header("authorization", format!("Bearer {key}").as_str()))
                .respond_with(response)
                .mount(&server)
                .await;
        }
        server
    }

    /// `server` with the primary key and `retries` attempts.
    pub(crate) fn network(server: &MockServer, retries: u8) -> NetworkConfig {
        let url: url::Url = server.uri().parse().unwrap();
        NetworkConfig {
            rpc_endpoints: vec![RPCEndpoint::new(url.clone())
                .with_api_key(PRIMARY.to_owned())
                .with_retries(retries)],
            ..NetworkConfig::from_rpc_url("test", url)
        }
    }

    pub(crate) fn with_backup(server: &MockServer) -> NearRpc {
        NearRpc::with_backup_api_key(network(server, 2), BACKUP)
    }

    pub(crate) async fn sent_with(server: &MockServer, key: &str) -> usize {
        let expected = format!("Bearer {key}");
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| {
                request.headers.iter().any(|(name, values)| {
                    name.as_str().eq_ignore_ascii_case("authorization")
                        && values.iter().any(|value| value.as_str() == expected)
                })
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use near_api::{
        errors::QueryError,
        types::{crypto::secret_key::ED25519SecretKey, Reference},
        Account, NetworkConfig, PublicKey, RPCEndpoint, SecretKey, Signer,
    };
    use near_openapi_client::types::RpcQueryError;
    use rstest::rstest;
    use wiremock::ResponseTemplate;

    use super::{
        keyed_endpoint::{network, sent_with, serving as endpoint, with_backup, BACKUP, PRIMARY},
        NearRpc, PrimaryPause, RefusedKey,
    };

    const ACCESS_KEY: &str = r#"{"jsonrpc":"2.0","id":"0","result":{"nonce":5,"permission":"FullAccess","block_height":1,"block_hash":"11111111111111111111111111111111"}}"#;

    fn secret_key() -> SecretKey {
        SecretKey::ED25519(ED25519SecretKey::from_secret_key([1; 32]))
    }

    fn public_key() -> PublicKey {
        secret_key().public_key()
    }

    fn answer() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(ACCESS_KEY)
    }

    async fn view_access_key(rpc: &NearRpc) -> Result<u64, QueryError<RpcQueryError>> {
        rpc.fetch(|| {
            Account("alice.near".parse().unwrap())
                .access_key(public_key())
                .at(Reference::Final)
        })
        .await
        .map(|key| key.data.nonce.0)
    }

    #[rstest]
    #[case::unauthorized(401)]
    #[case::payment_required(402)]
    #[case::forbidden(403)]
    #[tokio::test]
    async fn a_refused_primary_key_fails_over_to_the_backup(#[case] status: u16) {
        let server = endpoint(ResponseTemplate::new(status), answer()).await;

        assert_eq!(view_access_key(&with_backup(&server)).await.unwrap(), 5);
        assert_eq!(
            sent_with(&server, PRIMARY).await,
            1,
            "no retry on a refused key"
        );
        assert_eq!(sent_with(&server, BACKUP).await, 1);
    }

    #[tokio::test]
    async fn a_healthy_primary_never_sends_the_backup_key() {
        let server = endpoint(answer(), answer()).await;

        view_access_key(&with_backup(&server)).await.unwrap();
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
        assert_eq!(sent_with(&server, BACKUP).await, 0);
    }

    #[rstest]
    #[case::rate_limited(429)]
    #[case::unavailable(503)]
    #[tokio::test]
    async fn transient_failures_never_reach_the_backup(#[case] status: u16) {
        let server = endpoint(ResponseTemplate::new(status), answer()).await;

        assert!(view_access_key(&with_backup(&server)).await.is_err());
        assert_eq!(sent_with(&server, BACKUP).await, 0);
    }

    #[tokio::test]
    async fn without_a_backup_a_refusal_is_returned_after_one_request() {
        let server = endpoint(ResponseTemplate::new(402), answer()).await;
        let rpc = NearRpc::from(network(&server, 2));

        let error = view_access_key(&rpc).await.unwrap_err();
        assert_eq!(error.refused_status(), Some(402));
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
    }

    #[tokio::test]
    async fn a_refused_primary_is_skipped_while_paused() {
        let server = endpoint(ResponseTemplate::new(402), answer()).await;
        let rpc = with_backup(&server);

        view_access_key(&rpc).await.unwrap();
        view_access_key(&rpc).await.unwrap();
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
        assert_eq!(sent_with(&server, BACKUP).await, 2);
    }

    #[tokio::test]
    async fn the_primary_is_tried_again_once_the_pause_ends() {
        let server = endpoint(ResponseTemplate::new(402), answer()).await;
        let rpc = NearRpc {
            pause: Arc::new(PrimaryPause::new(Duration::ZERO)),
            ..with_backup(&server)
        };

        view_access_key(&rpc).await.unwrap();
        view_access_key(&rpc).await.unwrap();
        assert_eq!(sent_with(&server, PRIMARY).await, 2);
        assert_eq!(sent_with(&server, BACKUP).await, 2);
    }

    #[tokio::test]
    async fn a_paused_primary_that_answers_ends_the_pause() {
        let server = endpoint(answer(), ResponseTemplate::new(402)).await;
        let rpc = with_backup(&server);
        rpc.pause.start();

        assert_eq!(view_access_key(&rpc).await.unwrap(), 5);
        assert!(!rpc.pause.is_active());
        assert_eq!(sent_with(&server, BACKUP).await, 1);
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
    }

    #[tokio::test]
    async fn both_keys_refused_is_a_refusal() {
        let server = endpoint(ResponseTemplate::new(402), ResponseTemplate::new(401)).await;

        let error = view_access_key(&with_backup(&server)).await.unwrap_err();
        assert_eq!(error.refused_status(), Some(401));
    }

    #[test]
    fn a_refusal_is_reported_once_per_pause() {
        let pause = PrimaryPause::new(Duration::from_secs(60));
        assert!(pause.start());
        assert!(!pause.start());

        let expired = PrimaryPause::new(Duration::ZERO);
        assert!(expired.start());
        assert!(expired.start());
    }

    #[tokio::test]
    async fn keys_stay_out_of_debug_output_and_errors() {
        let server = endpoint(ResponseTemplate::new(402), ResponseTemplate::new(402)).await;
        let rpc = with_backup(&server);

        let error = view_access_key(&rpc).await.unwrap_err();
        for rendered in [format!("{rpc:?}"), error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains(PRIMARY), "{rendered}");
            assert!(!rendered.contains(BACKUP), "{rendered}");
        }
    }

    #[tokio::test]
    async fn an_endpoint_allowed_no_retries_still_makes_one_request() {
        let server = endpoint(answer(), answer()).await;

        view_access_key(&NearRpc::from(network(&server, 0)))
            .await
            .unwrap();
        assert_eq!(sent_with(&server, PRIMARY).await, 1);
    }

    #[tokio::test]
    async fn nonce_allocation_fails_over_to_the_backup() {
        let server = endpoint(ResponseTemplate::new(402), answer()).await;
        let signer = Signer::from_secret_key(secret_key()).unwrap();

        let (nonce, _, _) = with_backup(&server)
            .fetch_tx_nonce(&signer, &"alice.near".parse().unwrap(), public_key())
            .await
            .unwrap();
        assert_eq!(nonce, 6);
        assert_eq!(sent_with(&server, BACKUP).await, 1);
    }

    #[tokio::test]
    async fn per_endpoint_views_keep_the_backup_key() {
        let server = endpoint(ResponseTemplate::new(402), answer()).await;
        let mut network = network(&server, 2);
        network.rpc_endpoints.push(network.rpc_endpoints[0].clone());
        let views = NearRpc::with_backup_api_key(network, BACKUP).per_endpoint();

        assert_eq!(views.len(), 2);
        for view in &views {
            view_access_key(view).await.unwrap();
        }
        assert_eq!(
            sent_with(&server, PRIMARY).await,
            1,
            "one refusal pauses both"
        );
        assert_eq!(sent_with(&server, BACKUP).await, 2);
    }

    #[test]
    fn the_backup_key_goes_only_where_the_primary_sends_a_key() {
        let url: url::Url = "https://rpc.example.com".parse().unwrap();
        let mut network = NetworkConfig::from_rpc_url("test", url.clone());
        network.rpc_endpoints = vec![
            RPCEndpoint::new(url.clone()).with_api_key(PRIMARY.to_owned()),
            RPCEndpoint::new(url),
        ];

        let rpc = NearRpc::with_backup_api_key(network, BACKUP);
        let headers: Vec<_> = rpc
            .backup
            .as_deref()
            .unwrap()
            .rpc_endpoints
            .iter()
            .map(|endpoint| endpoint.bearer_header.clone())
            .collect();
        assert_eq!(headers, [Some(format!("Bearer {BACKUP}")), None]);
    }

    #[test]
    fn a_backup_key_without_a_primary_key_is_dropped() {
        let keyless =
            NetworkConfig::from_rpc_url("test", "https://rpc.example.com".parse().unwrap());

        assert!(
            format!("{:?}", NearRpc::with_backup_api_key(keyless, BACKUP))
                .contains("backup_key: false")
        );
    }

    #[test]
    fn a_key_over_plain_http_is_cleartext() {
        let http = |key: Option<&str>| {
            let url: url::Url = "http://rpc.example.com".parse().unwrap();
            let mut network = NetworkConfig::from_rpc_url("test", url);
            if let Some(key) = key {
                network.rpc_endpoints[0] = network.rpc_endpoints[0]
                    .clone()
                    .with_api_key(key.to_owned());
            }
            network
        };

        assert!(!NearRpc::from(http(None)).sends_key_in_cleartext());
        assert!(NearRpc::from(http(Some(PRIMARY))).sends_key_in_cleartext());
        assert!(NearRpc::with_backup_api_key(http(Some(PRIMARY)), BACKUP).sends_key_in_cleartext());
    }
}
