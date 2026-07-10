use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pyth_lazer_protocol::api::{Channel, SubscriptionId};
use templar_gateway_core::{OraclePayloadSource, RedactedString};
use thiserror::Error;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue},
        protocol::WebSocketConfig,
        Message,
    },
    MaybeTlsStream, WebSocketStream,
};
use url::Url;

use crate::lazer_wire::{
    decode_stream_message, subscription_frame_for_feeds, unsubscription_frame, DecodedLazerPayload,
    LazerStreamEvent, MAX_STREAM_JSON_MESSAGE_BYTES,
};

const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const MAX_ACTIVE_SUBSCRIPTIONS: usize = 128;
/// A subscribed stream delivers at least once per channel interval, so prolonged
/// silence means the connection is dead. Stays well under the 60s after which the
/// server closes a connection, so our own timeout can't race theirs.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long [`LazerPayloadSource::fetch_payload`] waits for a payload covering its
/// feeds — long enough to absorb a reconnect. Distinct from `max_payload_age`, which
/// bounds how old an already-cached payload may be when it is served.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod mock_server;
#[cfg(test)]
mod tests;

type LazerStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub struct LazerSourceConfig {
    ws_url: Url,
    api_token: RedactedString,
    channel: Channel,
    max_payload_age: Duration,
}

#[derive(Debug, Clone)]
pub struct LazerSubscriptionConfig {
    pub channel: String,
    pub max_payload_age: Duration,
}

impl LazerSourceConfig {
    pub fn new(
        ws_url: Url,
        api_token: RedactedString,
        subscription: LazerSubscriptionConfig,
    ) -> LazerResult<Self> {
        if ws_url.scheme() != "wss" {
            return Err(LazerClientError::InsecureWebSocketUrl);
        }
        if api_token.trim().is_empty() {
            return Err(LazerClientError::EmptyApiToken);
        }
        if subscription.max_payload_age.is_zero() {
            return Err(LazerClientError::InvalidMaxPayloadAge);
        }
        let max_payload_age = subscription.max_payload_age;
        let channel = parse_channel(subscription.channel)?;
        Ok(Self {
            ws_url,
            api_token,
            channel,
            max_payload_age,
        })
    }

    pub(crate) fn channel(&self) -> Channel {
        self.channel
    }

    /// Test-only: point the source at the in-process mock server, which speaks
    /// plaintext `ws://`. Production configs must be `wss://` — see [`Self::new`].
    #[cfg(test)]
    pub(crate) fn for_mock_server(ws_url: Url, max_payload_age: Duration) -> Self {
        Self {
            ws_url,
            api_token: RedactedString::from("test-token"),
            channel: parse_channel("fixed_rate@200ms".to_owned()).expect("valid channel"),
            max_payload_age,
        }
    }
}

fn parse_channel(channel: String) -> LazerResult<Channel> {
    serde_json::from_value(serde_json::Value::String(channel.clone()))
        .map_err(|_| LazerClientError::InvalidChannel(channel))
}

#[derive(Debug, Error)]
pub enum LazerClientError {
    #[error("Pyth Lazer websocket URL must use wss://")]
    InsecureWebSocketUrl,
    #[error("Pyth Lazer API token must not be empty")]
    EmptyApiToken,
    #[error("unsupported Pyth Lazer channel: {0}")]
    InvalidChannel(String),
    #[error("Pyth Lazer max payload age must be greater than zero")]
    InvalidMaxPayloadAge,
    #[error("Pyth Lazer request failed: {0}")]
    Request(String),
    #[error("Pyth Lazer stream message is missing solana payload")]
    MissingSolanaPayload,
    #[error("Pyth Lazer solana payload decode failed: {0}")]
    Decode(String),
    #[error("Pyth Lazer solana payload exceeds size limit")]
    PayloadTooLarge,
    #[error("Pyth Lazer payload request must include at least one feed id")]
    EmptyRequest,
    #[error("timed out waiting for a Pyth Lazer payload")]
    Timeout,
    #[error("Pyth Lazer subscription failed: {0}")]
    SubscriptionFailed(String),
}

pub type LazerResult<T> = Result<T, LazerClientError>;

fn source_stopped() -> LazerClientError {
    LazerClientError::Request("Pyth Lazer source task stopped".to_owned())
}

/// The latest state of one subscription, published to every waiting `fetch_payload`.
#[derive(Debug, Clone)]
enum Slot {
    /// Subscribed, but no payload has arrived yet.
    Waiting,
    /// The server rejected the subscription. Terminal: invalid feeds never become valid.
    Failed(String),
    Ready(CachedPayload),
}

#[derive(Debug, Clone)]
struct CachedPayload {
    payload: Vec<u8>,
    feed_ids: BTreeSet<u32>,
    received_at: Instant,
}

/// Ask the stream task for the slot of a subscription covering `feed_ids`, creating one if needed.
#[derive(Debug)]
struct SubscribeRequest {
    feed_ids: BTreeSet<u32>,
    slot_tx: oneshot::Sender<watch::Receiver<Slot>>,
}

#[derive(Debug, Clone)]
pub struct LazerPayloadSource {
    max_payload_age: Duration,
    subscribe_tx: mpsc::Sender<SubscribeRequest>,
    _task: Arc<TaskGuard>,
}

#[derive(Debug)]
struct TaskGuard(JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl LazerPayloadSource {
    pub fn spawn(config: LazerSourceConfig) -> Self {
        let max_payload_age = config.max_payload_age;
        let (subscribe_tx, subscribe_rx) = mpsc::channel(32);
        let task = tokio::spawn(StreamTask::new(config).run(subscribe_rx));
        Self {
            max_payload_age,
            subscribe_tx,
            _task: Arc::new(TaskGuard(task)),
        }
    }

    async fn subscribe(&self, feed_ids: BTreeSet<u32>) -> LazerResult<watch::Receiver<Slot>> {
        let (slot_tx, slot_rx) = oneshot::channel();
        self.subscribe_tx
            .send(SubscribeRequest { feed_ids, slot_tx })
            .await
            .map_err(|_| source_stopped())?;
        slot_rx.await.map_err(|_| source_stopped())
    }
}

#[async_trait]
impl OraclePayloadSource for LazerPayloadSource {
    type PriceId = u32;
    type Error = LazerClientError;

    async fn fetch_payload(&self, price_ids: &[Self::PriceId]) -> Result<Vec<u8>, Self::Error> {
        if price_ids.is_empty() {
            return Err(LazerClientError::EmptyRequest);
        }
        let feed_ids: BTreeSet<u32> = price_ids.iter().copied().collect();

        // One deadline over both halves: the stream task may be mid-connect when the request
        // arrives, so waiting for it to answer is as unbounded as waiting for a payload.
        timeout(FETCH_TIMEOUT, async {
            // Sending the request also wakes the stream task out of any reconnect backoff, so
            // a fetch never waits on a sleep it could have cancelled.
            let mut slot = self.subscribe(feed_ids.clone()).await?;
            await_payload(&mut slot, &feed_ids, self.max_payload_age).await
        })
        .await
        .map_err(|_| LazerClientError::Timeout)?
    }
}

async fn await_payload(
    slot: &mut watch::Receiver<Slot>,
    feed_ids: &BTreeSet<u32>,
    max_payload_age: Duration,
) -> LazerResult<Vec<u8>> {
    loop {
        // Scope the borrow: a `watch::Ref` must not be held across an await.
        let settled = ready_payload(&slot.borrow_and_update(), feed_ids, max_payload_age);
        if let Some(result) = settled {
            return result;
        }
        slot.changed().await.map_err(|_| source_stopped())?;
    }
}

/// `None` while the subscription has yet to produce a payload we can serve.
fn ready_payload(
    slot: &Slot,
    feed_ids: &BTreeSet<u32>,
    max_payload_age: Duration,
) -> Option<LazerResult<Vec<u8>>> {
    match slot {
        Slot::Failed(error) => Some(Err(LazerClientError::SubscriptionFailed(error.clone()))),
        Slot::Ready(cached)
            if cached.received_at.elapsed() <= max_payload_age
                && feed_ids.is_subset(&cached.feed_ids) =>
        {
            Some(Ok(cached.payload.clone()))
        }
        // A stale or not-yet-covering payload is not a failure: on a live stream the
        // next one is a channel interval away. Wait for it rather than erroring.
        Slot::Waiting | Slot::Ready(_) => None,
    }
}

/// The background task that owns the websocket and every subscription on it. Callers
/// reach it only by message, so the subscription set needs no lock and cannot be observed
/// mid-update.
///
/// Not an `actix::Actor`: this is a plain tokio task owning a long-lived resource, so it
/// stays usable from the lock-free `templar-gateway-client` path, which runs no actix
/// `System`. The gateway's actix actors are request/response mailboxes at the RPC edge.
struct StreamTask {
    config: LazerSourceConfig,
    /// Keyed by a monotonically increasing id, so the first key is the oldest
    /// subscription — which is the one eviction takes.
    subscriptions: BTreeMap<SubscriptionId, Subscription>,
    next_id: u64,
    backoff: Duration,
}

struct Subscription {
    feed_ids: BTreeSet<u32>,
    slot: watch::Sender<Slot>,
}

/// What [`StreamTask::register`] changed, and therefore which frames the stream owes the
/// server. Empty when an existing subscription already covered the request.
#[derive(Debug, Default)]
struct Registration {
    new_subscription: Option<SubscriptionId>,
    evicted: Vec<SubscriptionId>,
}

/// A stream that ended without an error, and so without a reconnect.
#[derive(Debug)]
enum StreamEnd {
    /// Every `LazerPayloadSource` was dropped.
    Shutdown,
    /// The last subscription went away, so the connection has no purpose.
    NoSubscriptions,
}

impl StreamTask {
    fn new(config: LazerSourceConfig) -> Self {
        Self {
            config,
            subscriptions: BTreeMap::new(),
            next_id: 1,
            backoff: INITIAL_RECONNECT_BACKOFF,
        }
    }

    async fn run(mut self, mut requests: mpsc::Receiver<SubscribeRequest>) {
        loop {
            // With no subscription there is nothing to stream, and the server closes a
            // subscription-less connection after 60s — an idle connection is just a
            // reconnect loop. Hold none, and wait for demand instead.
            if self.subscriptions.is_empty() {
                let Some(request) = requests.recv().await else {
                    return;
                };
                self.register(request);
            }

            match self.connect_and_stream(&mut requests).await {
                Ok(StreamEnd::Shutdown) => return,
                // Nothing left to stream: loop back around and park, rather than
                // hold a connection the server would close anyway.
                Ok(StreamEnd::NoSubscriptions) => continue,
                Err(error) => tracing::warn!(
                    %error,
                    subscriptions = self.subscriptions.len(),
                    "Pyth Lazer stream disconnected",
                ),
            }

            if !self.wait_before_reconnect(&mut requests).await {
                return;
            }
        }
    }

    /// Sleep out the reconnect backoff, waking early for a subscribe request. Returns
    /// `false` once every sender is gone, which is the shutdown signal.
    async fn wait_before_reconnect(
        &mut self,
        requests: &mut mpsc::Receiver<SubscribeRequest>,
    ) -> bool {
        let sleep = self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_RECONNECT_BACKOFF);
        tokio::select! {
            () = tokio::time::sleep(sleep) => true,
            request = requests.recv() => match request {
                // Registered now, subscribed by the replay on the next connect.
                Some(request) => { self.register(request); true }
                None => false,
            },
        }
    }

    /// Streams until the connection ends. `Ok` is an orderly end (see [`StreamEnd`]);
    /// every disconnect is an `Err`, which the caller retries after a backoff.
    async fn connect_and_stream(
        &mut self,
        requests: &mut mpsc::Receiver<SubscribeRequest>,
    ) -> LazerResult<StreamEnd> {
        let mut stream = self.connect().await?;
        self.replay_subscriptions(&mut stream).await?;

        loop {
            // Every subscription was rejected: drop the connection instead of letting
            // it idle. A connection exists exactly while a subscription does.
            if self.subscriptions.is_empty() {
                return Ok(StreamEnd::NoSubscriptions);
            }

            tokio::select! {
                message = timeout(STREAM_IDLE_TIMEOUT, stream.next()) => {
                    let message = message
                        .map_err(|_| LazerClientError::Request("websocket idle timeout".to_owned()))?;
                    let Some(message) = message else {
                        return Err(LazerClientError::Request("websocket closed".to_owned()));
                    };
                    let message = message.map_err(|error| LazerClientError::Request(error.to_string()))?;
                    let Message::Text(text) = message else {
                        continue;
                    };
                    match decode_stream_message(text.as_ref()) {
                        Ok(event) => self.handle_event(event),
                        Err(error) => tracing::warn!(%error, "ignored invalid Pyth Lazer stream message"),
                    }
                }
                request = requests.recv() => {
                    let Some(request) = request else {
                        return Ok(StreamEnd::Shutdown);
                    };
                    let registration = self.register(request);
                    self.send_registration(&mut stream, &registration).await?;
                }
            }
        }
    }

    async fn connect(&self) -> LazerResult<LazerStream> {
        let mut request = self
            .config
            .ws_url
            .as_str()
            .into_client_request()
            .map_err(|error| LazerClientError::Request(error.to_string()))?;
        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", self.config.api_token.as_ref()))
                .map_err(|error| LazerClientError::Request(error.to_string()))?;
        request.headers_mut().insert(AUTHORIZATION, authorization);

        let (stream, _) = connect_async_with_config(
            request,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(MAX_STREAM_JSON_MESSAGE_BYTES))
                    .max_frame_size(Some(MAX_STREAM_JSON_MESSAGE_BYTES)),
            ),
            false,
        )
        .await
        .map_err(|error| LazerClientError::Request(error.to_string()))?;
        Ok(stream)
    }

    /// Resubscribe every live subscription on a fresh connection. A request that
    /// arrived while disconnected is already registered, so it is replayed here too —
    /// there is no queued frame that could subscribe the same id twice.
    ///
    /// Cached payloads survive the reconnect: `max_payload_age` already decides whether
    /// one is still servable, so a brief reconnect need not stall a fetch.
    async fn replay_subscriptions(&self, stream: &mut LazerStream) -> LazerResult<()> {
        for (id, subscription) in &self.subscriptions {
            let frame =
                subscription_frame_for_feeds(&self.config, *id, subscription.feed_ids.clone())?;
            send_frame(stream, frame).await?;
        }
        Ok(())
    }

    async fn send_registration(
        &self,
        stream: &mut LazerStream,
        registration: &Registration,
    ) -> LazerResult<()> {
        for id in &registration.evicted {
            send_frame(stream, unsubscription_frame(*id)?).await?;
        }
        if let Some(id) = registration.new_subscription {
            let Some(subscription) = self.subscriptions.get(&id) else {
                return Ok(());
            };
            let frame =
                subscription_frame_for_feeds(&self.config, id, subscription.feed_ids.clone())?;
            send_frame(stream, frame).await?;
        }
        Ok(())
    }

    /// Hand the caller the slot of a subscription covering `feed_ids`, creating one if
    /// no live subscription already does.
    fn register(&mut self, request: SubscribeRequest) -> Registration {
        let SubscribeRequest { feed_ids, slot_tx } = request;

        if let Some(subscription) = self
            .subscriptions
            .values()
            .find(|subscription| feed_ids.is_subset(&subscription.feed_ids))
        {
            let _ = slot_tx.send(subscription.slot.subscribe());
            return Registration::default();
        }

        let evicted = self.evict_to_fit();
        let id = SubscriptionId(self.next_id);
        self.next_id += 1;
        let (slot, slot_rx) = watch::channel(Slot::Waiting);
        let _ = slot_tx.send(slot_rx);
        self.subscriptions
            .insert(id, Subscription { feed_ids, slot });

        Registration {
            new_subscription: Some(id),
            evicted,
        }
    }

    /// Ids increase monotonically, so the first key is the oldest subscription.
    fn evict_to_fit(&mut self) -> Vec<SubscriptionId> {
        let mut evicted = Vec::new();
        while self.subscriptions.len() >= MAX_ACTIVE_SUBSCRIPTIONS {
            let Some(&oldest) = self.subscriptions.keys().next() else {
                break;
            };
            self.fail(oldest, "subscription evicted".to_owned());
            evicted.push(oldest);
        }
        evicted
    }

    fn handle_event(&mut self, event: LazerStreamEvent) {
        match event {
            LazerStreamEvent::Payload(payload) => self.cache_payload(payload),
            // Nothing to do: a fetch waits for the payload, not for the acknowledgement.
            LazerStreamEvent::Subscribed { .. } => {}
            LazerStreamEvent::SubscribedWithInvalidFeedIdsIgnored {
                subscription_id,
                subscribed_feed_ids,
            } => {
                let covered =
                    self.subscriptions
                        .get(&subscription_id)
                        .is_some_and(|subscription| {
                            subscription.feed_ids.is_subset(&subscribed_feed_ids)
                        });
                if !covered {
                    self.fail(
                        subscription_id,
                        "subscription ignored requested feed ids".to_owned(),
                    );
                }
            }
            LazerStreamEvent::SubscriptionError {
                subscription_id,
                error,
            } => self.fail(subscription_id, error),
            LazerStreamEvent::Unsubscribed { subscription_id } => tracing::debug!(
                subscription_id = subscription_id.0,
                "Pyth Lazer subscription removed"
            ),
            LazerStreamEvent::Error { error } => {
                tracing::warn!(%error, "Pyth Lazer stream returned an error response");
            }
        }
    }

    fn cache_payload(&mut self, payload: DecodedLazerPayload) {
        self.backoff = INITIAL_RECONNECT_BACKOFF;
        let Some(subscription) = self.subscriptions.get(&payload.subscription_id) else {
            tracing::warn!(
                subscription_id = payload.subscription_id.0,
                "ignored Pyth Lazer payload for unknown subscription"
            );
            return;
        };
        subscription.slot.send_replace(Slot::Ready(CachedPayload {
            payload: payload.bytes,
            feed_ids: payload.feed_ids,
            received_at: Instant::now(),
        }));
    }

    /// Drop a subscription and wake its waiters with the reason. Terminal, so it is
    /// removed rather than left to be replayed on the next connect.
    fn fail(&mut self, subscription_id: SubscriptionId, error: String) {
        if let Some(subscription) = self.subscriptions.remove(&subscription_id) {
            subscription.slot.send_replace(Slot::Failed(error));
        }
    }
}

async fn send_frame(stream: &mut LazerStream, frame: String) -> LazerResult<()> {
    stream
        .send(Message::Text(frame.into()))
        .await
        .map_err(|error| LazerClientError::Request(error.to_string()))
}
