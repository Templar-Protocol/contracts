use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pyth_lazer_protocol::api::{Channel, SubscriptionId};
use templar_gateway_core::{OraclePayloadSource, RedactedString};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot, watch, RwLock},
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
};
use url::Url;

use crate::lazer_wire::{
    decode_stream_message, subscription_frame_for_feeds, unsubscription_frame, LazerStreamEvent,
    MAX_STREAM_JSON_MESSAGE_BYTES,
};

const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const MAX_ACTIVE_SUBSCRIPTIONS: usize = 128;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod tests;

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
    #[error("Pyth Lazer cache does not yet contain a payload")]
    CacheMiss,
    #[error("Pyth Lazer payload request must include at least one feed id")]
    EmptyRequest,
    #[error("Pyth Lazer cached payload is stale")]
    StalePayload,
    #[error("Pyth Lazer cached payload does not cover requested feeds")]
    FeedNotCovered,
    #[error("Pyth Lazer subscription failed: {0}")]
    SubscriptionFailed(String),
}

pub type LazerResult<T> = Result<T, LazerClientError>;

#[derive(Debug, Clone)]
pub struct LazerPayloadSource {
    inner: Arc<LazerPayloadSourceInner>,
    subscribe_tx: mpsc::Sender<SubscribeRequest>,
    _task: Arc<TaskGuard>,
}

#[derive(Debug)]
struct LazerPayloadSourceInner {
    config: LazerSourceConfig,
    subscriptions: RwLock<SubscriptionState>,
}

#[derive(Debug)]
struct SubscriptionState {
    active: BTreeMap<SubscriptionId, SubscriptionInfo>,
    pending: BTreeMap<SubscriptionId, PendingSubscription>,
    fifo: VecDeque<SubscriptionId>,
    next_id: u64,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        Self {
            active: BTreeMap::new(),
            pending: BTreeMap::new(),
            fifo: VecDeque::new(),
            next_id: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct SubscriptionInfo {
    feed_ids: BTreeSet<u32>,
    cache: watch::Receiver<Option<CachedPayload>>,
    cache_tx: Arc<watch::Sender<Option<CachedPayload>>>,
}

#[derive(Debug)]
struct TaskGuard {
    /// `None` only for the `#[cfg(test)] from_cached` seam, which runs no background task.
    task: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
struct CachedPayload {
    payload: Vec<u8>,
    feed_ids: BTreeSet<u32>,
    received_at: Instant,
}

#[derive(Debug)]
struct PendingSubscription {
    response_tx: oneshot::Sender<LazerResult<SubscriptionId>>,
}

struct SubscribeRequest {
    subscription_id: SubscriptionId,
    feed_ids: BTreeSet<u32>,
    evicted_ids: Vec<SubscriptionId>,
}

impl LazerPayloadSource {
    pub fn spawn(config: LazerSourceConfig) -> Self {
        let inner = Arc::new(LazerPayloadSourceInner {
            config,
            subscriptions: RwLock::new(SubscriptionState::default()),
        });
        let (subscribe_tx, subscribe_rx) = mpsc::channel(32);
        let task_inner = Arc::clone(&inner);
        let handle = tokio::spawn(async move { task_inner.run(subscribe_rx).await });
        Self {
            inner,
            subscribe_tx,
            _task: Arc::new(TaskGuard { task: Some(handle) }),
        }
    }

    #[cfg(test)]
    fn from_cached(config: LazerSourceConfig, payload: Option<CachedPayload>) -> Self {
        let (subscribe_tx, _) = mpsc::channel(32);
        let mut subscriptions = SubscriptionState::default();
        let (cache_tx, cache_rx) = watch::channel(payload.clone());
        let feed_ids = payload.map(|p| p.feed_ids).unwrap_or_default();
        subscriptions.active.insert(
            SubscriptionId(1),
            SubscriptionInfo {
                feed_ids,
                cache: cache_rx,
                cache_tx: Arc::new(cache_tx),
            },
        );
        subscriptions.fifo.push_back(SubscriptionId(1));
        subscriptions.next_id = 2;
        Self {
            inner: Arc::new(LazerPayloadSourceInner {
                config,
                subscriptions: RwLock::new(subscriptions),
            }),
            subscribe_tx,
            _task: Arc::new(TaskGuard { task: None }),
        }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
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

        let requested_feeds: BTreeSet<u32> = price_ids.iter().copied().collect();

        let subscription_id = self.ensure_subscription(requested_feeds.clone()).await?;

        let mut cache_rx = self.cache_receiver(subscription_id).await?;
        if let Some(payload) = self.payload_from_cache(&cache_rx, &requested_feeds)? {
            return Ok(payload);
        }

        let wait_for_payload = async {
            loop {
                cache_rx
                    .changed()
                    .await
                    .map_err(|_| LazerClientError::CacheMiss)?;
                if let Some(payload) = self.payload_from_cache(&cache_rx, &requested_feeds)? {
                    return Ok(payload);
                }
            }
        };

        timeout(self.inner.config.max_payload_age, wait_for_payload)
            .await
            .map_err(|_| LazerClientError::CacheMiss)?
    }
}

impl LazerPayloadSource {
    async fn cache_receiver(
        &self,
        subscription_id: SubscriptionId,
    ) -> LazerResult<watch::Receiver<Option<CachedPayload>>> {
        let subscriptions = self.inner.subscriptions.read().await;
        subscriptions
            .active
            .get(&subscription_id)
            .map(|subscription| subscription.cache.clone())
            .ok_or(LazerClientError::CacheMiss)
    }

    fn payload_from_cache(
        &self,
        cache_rx: &watch::Receiver<Option<CachedPayload>>,
        requested_feeds: &BTreeSet<u32>,
    ) -> LazerResult<Option<Vec<u8>>> {
        let Some(cached) = cache_rx.borrow().clone() else {
            return Ok(None);
        };

        if cached.received_at.elapsed() > self.inner.config.max_payload_age {
            return Err(LazerClientError::StalePayload);
        }

        if !requested_feeds.is_subset(&cached.feed_ids) {
            return Err(LazerClientError::FeedNotCovered);
        }

        Ok(Some(cached.payload))
    }

    async fn ensure_subscription(&self, feed_ids: BTreeSet<u32>) -> LazerResult<SubscriptionId> {
        {
            let subscriptions = self.inner.subscriptions.read().await;
            for (id, info) in &subscriptions.active {
                if !subscriptions.pending.contains_key(id) && feed_ids.is_subset(&info.feed_ids) {
                    return Ok(*id);
                }
            }
        }

        let (response_tx, response_rx) = oneshot::channel();
        let (subscription_id, evicted_ids) = self
            .inner
            .register_pending_subscription(feed_ids.clone(), response_tx)
            .await;
        if self
            .subscribe_tx
            .send(SubscribeRequest {
                subscription_id,
                feed_ids,
                evicted_ids,
            })
            .await
            .is_err()
        {
            self.inner
                .fail_pending_subscription(
                    subscription_id,
                    "subscription channel closed".to_owned(),
                )
                .await;
            return Err(LazerClientError::Request(
                "subscription channel closed".to_owned(),
            ));
        }

        match timeout(self.inner.config.max_payload_age, response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LazerClientError::Request(
                "subscription response channel closed".to_owned(),
            )),
            Err(_) => {
                self.inner
                    .fail_pending_subscription(
                        subscription_id,
                        "subscription acknowledgement timed out".to_owned(),
                    )
                    .await;
                Err(LazerClientError::SubscriptionFailed(
                    "subscription acknowledgement timed out".to_owned(),
                ))
            }
        }
    }
}

impl LazerPayloadSourceInner {
    async fn register_pending_subscription(
        &self,
        feed_ids: BTreeSet<u32>,
        response_tx: oneshot::Sender<LazerResult<SubscriptionId>>,
    ) -> (SubscriptionId, Vec<SubscriptionId>) {
        let mut subscriptions = self.subscriptions.write().await;
        let subscription_id = SubscriptionId(subscriptions.next_id);
        subscriptions.next_id += 1;

        let mut evicted_ids = Vec::new();
        while subscriptions.active.len() >= MAX_ACTIVE_SUBSCRIPTIONS {
            let Some(evicted_id) = subscriptions.fifo.pop_front() else {
                break;
            };
            subscriptions.active.remove(&evicted_id);
            if let Some(pending) = subscriptions.pending.remove(&evicted_id) {
                let _ = pending
                    .response_tx
                    .send(Err(LazerClientError::SubscriptionFailed(
                        "subscription evicted before acknowledgement".to_owned(),
                    )));
            }
            evicted_ids.push(evicted_id);
        }

        let (cache_tx, cache_rx) = watch::channel(None);
        subscriptions.active.insert(
            subscription_id,
            SubscriptionInfo {
                feed_ids,
                cache: cache_rx,
                cache_tx: Arc::new(cache_tx),
            },
        );
        subscriptions.fifo.push_back(subscription_id);
        subscriptions
            .pending
            .insert(subscription_id, PendingSubscription { response_tx });

        (subscription_id, evicted_ids)
    }

    async fn run(self: Arc<Self>, mut subscribe_rx: mpsc::Receiver<SubscribeRequest>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self
                .connect_and_stream(&mut subscribe_rx, &mut backoff)
                .await
            {
                // The subscribe channel closed: the `LazerPayloadSource` was dropped, so shut the
                // task down cleanly rather than reconnecting.
                Ok(()) => break,
                Err(error) => {
                    self.fail_pending_subscriptions("websocket disconnected")
                        .await;
                    tracing::warn!(%error, "Pyth Lazer stream disconnected");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RECONNECT_BACKOFF);
                }
            }
        }
    }

    /// Connect, stream, and dispatch until the connection ends. Returns `Ok(())` only when the
    /// subscribe channel closes (graceful shutdown); every disconnect/error returns `Err`, which
    /// the caller treats as a reconnect trigger.
    async fn connect_and_stream(
        &self,
        subscribe_rx: &mut mpsc::Receiver<SubscribeRequest>,
        backoff: &mut Duration,
    ) -> LazerResult<()> {
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

        let (mut stream, _) = connect_async_with_config(
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

        // Replay all active subscriptions after reconnect. Any `SubscribeRequest` still
        // queued for one of these ids (registered while the task was reconnecting) is now
        // redundant — the replay already (re)sent its frame — so those queued requests must
        // be skipped below to avoid subscribing the same id twice.
        let replayed = self.replay_active_subscriptions(&mut stream).await?;

        loop {
            tokio::select! {
                message = timeout(STREAM_IDLE_TIMEOUT, stream.next()) => {
                    let message = message.map_err(|_| LazerClientError::Request("websocket idle timeout".to_owned()))?;
                    let Some(message) = message else {
                        break;
                    };
                    let message = message.map_err(|error| LazerClientError::Request(error.to_string()))?;
                    let Message::Text(text) = message else {
                        continue;
                    };
                    match decode_stream_message(text.as_ref()) {
                        Ok(event) => self.handle_stream_event(event, backoff).await,
                        Err(error) => tracing::warn!(%error, "ignored invalid Pyth Lazer stream message"),
                    }
                }
                req = subscribe_rx.recv() => {
                    let Some(req) = req else {
                        // All senders dropped: the source is shutting down.
                        return Ok(());
                    };
                    self.handle_subscribe_request(&mut stream, req, &replayed).await?;
                }
            }
        }
        Err(LazerClientError::Request("websocket closed".to_owned()))
    }

    /// Resubscribe every active subscription after a reconnect. Returns the ids that were
    /// replayed so [`Self::handle_subscribe_request`] can drop any queued request for the
    /// same id (a subscription registered during the reconnect is both `active` and still
    /// has its `SubscribeRequest` queued, and must not be subscribed twice).
    async fn replay_active_subscriptions(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> LazerResult<HashSet<SubscriptionId>> {
        let active = {
            let subscriptions = self.subscriptions.read().await;
            subscriptions
                .active
                .iter()
                .map(|(id, info)| (*id, info.feed_ids.clone(), Arc::clone(&info.cache_tx)))
                .collect::<Vec<_>>()
        };

        let mut replayed = HashSet::with_capacity(active.len());
        for (id, feed_ids, cache_tx) in active {
            let frame = subscription_frame_for_feeds(&self.config, id, feed_ids)?;
            stream
                .send(Message::Text(frame.into()))
                .await
                .map_err(|error| LazerClientError::Request(error.to_string()))?;
            let _ = cache_tx.send(None);
            replayed.insert(id);
        }
        Ok(replayed)
    }

    async fn handle_subscribe_request(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        req: SubscribeRequest,
        replayed: &HashSet<SubscriptionId>,
    ) -> LazerResult<()> {
        // Skip a request whose subscription was just resubscribed by the reconnect replay:
        // sending it again would subscribe the same id twice and a duplicate-subscription
        // error could tear down the live subscription. Ids are never reused, so a replayed
        // subscription never legitimately needs a second subscribe frame.
        if replayed.contains(&req.subscription_id)
            || !self.subscription_is_pending(req.subscription_id).await
        {
            return Ok(());
        }

        for evicted_id in req.evicted_ids {
            let frame = unsubscription_frame(evicted_id)?;
            stream
                .send(Message::Text(frame.into()))
                .await
                .map_err(|error| LazerClientError::Request(error.to_string()))?;
        }

        let frame =
            subscription_frame_for_feeds(&self.config, req.subscription_id, req.feed_ids.clone())?;
        if let Err(error) = stream.send(Message::Text(frame.into())).await {
            let message = error.to_string();
            self.fail_pending_subscription(req.subscription_id, message.clone())
                .await;
            return Err(LazerClientError::Request(message));
        }
        Ok(())
    }

    async fn subscription_is_pending(&self, subscription_id: SubscriptionId) -> bool {
        let subscriptions = self.subscriptions.read().await;
        subscriptions.pending.contains_key(&subscription_id)
    }

    async fn handle_stream_event(&self, event: LazerStreamEvent, backoff: &mut Duration) {
        match event {
            LazerStreamEvent::Payload(payload) => {
                *backoff = Duration::from_secs(1);
                self.update_subscription_cache(payload).await;
            }
            LazerStreamEvent::Subscribed { subscription_id } => {
                self.acknowledge_subscription(subscription_id).await;
            }
            LazerStreamEvent::SubscribedWithInvalidFeedIdsIgnored {
                subscription_id,
                subscribed_feed_ids,
            } => {
                self.acknowledge_partial_subscription(subscription_id, subscribed_feed_ids)
                    .await;
            }
            LazerStreamEvent::SubscriptionError {
                subscription_id,
                error,
            } => {
                self.fail_pending_subscription(subscription_id, error).await;
            }
            LazerStreamEvent::Unsubscribed { subscription_id } => {
                tracing::debug!(
                    subscription_id = subscription_id.0,
                    "Pyth Lazer subscription removed"
                );
            }
            LazerStreamEvent::Error { error } => {
                tracing::warn!(%error, "Pyth Lazer stream returned an error response");
            }
        }
    }

    async fn acknowledge_subscription(&self, subscription_id: SubscriptionId) {
        let pending = {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.pending.remove(&subscription_id)
        };
        if let Some(pending) = pending {
            let _ = pending.response_tx.send(Ok(subscription_id));
        }
    }

    async fn acknowledge_partial_subscription(
        &self,
        subscription_id: SubscriptionId,
        subscribed_feed_ids: BTreeSet<u32>,
    ) {
        let result = {
            let mut subscriptions = self.subscriptions.write().await;
            let covers_requested = subscriptions
                .active
                .get(&subscription_id)
                .is_some_and(|info| info.feed_ids.is_subset(&subscribed_feed_ids));
            if covers_requested {
                subscriptions
                    .pending
                    .remove(&subscription_id)
                    .map(|pending| (pending, Ok(subscription_id)))
            } else {
                subscriptions.active.remove(&subscription_id);
                subscriptions.fifo.retain(|id| *id != subscription_id);
                subscriptions
                    .pending
                    .remove(&subscription_id)
                    .map(|pending| {
                        (
                            pending,
                            Err(LazerClientError::SubscriptionFailed(
                                "subscription ignored requested feed ids".to_owned(),
                            )),
                        )
                    })
            }
        };

        if let Some((pending, result)) = result {
            let _ = pending.response_tx.send(result);
        }
    }

    async fn fail_pending_subscription(&self, subscription_id: SubscriptionId, error: String) {
        let pending = {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.active.remove(&subscription_id);
            subscriptions.fifo.retain(|id| *id != subscription_id);
            subscriptions.pending.remove(&subscription_id)
        };
        if let Some(pending) = pending {
            let _ = pending
                .response_tx
                .send(Err(LazerClientError::SubscriptionFailed(error)));
        }
    }

    async fn fail_pending_subscriptions(&self, error: &str) {
        let pending = {
            let mut subscriptions = self.subscriptions.write().await;
            let ids = subscriptions.pending.keys().copied().collect::<Vec<_>>();
            for id in &ids {
                subscriptions.active.remove(id);
            }
            subscriptions
                .fifo
                .retain(|queued_id| !ids.contains(queued_id));
            std::mem::take(&mut subscriptions.pending)
                .into_values()
                .collect::<Vec<_>>()
        };

        for pending in pending {
            let _ = pending
                .response_tx
                .send(Err(LazerClientError::SubscriptionFailed(error.to_owned())));
        }
    }

    async fn update_subscription_cache(&self, payload: crate::lazer_wire::DecodedLazerPayload) {
        let subscriptions = self.subscriptions.read().await;
        let Some(info) = subscriptions.active.get(&payload.subscription_id) else {
            tracing::warn!(
                subscription_id = payload.subscription_id.0,
                "ignored Pyth Lazer payload for unknown subscription"
            );
            return;
        };
        let cached = CachedPayload {
            payload: payload.bytes,
            feed_ids: payload.feed_ids,
            received_at: Instant::now(),
        };
        if let Err(error) = info.cache_tx.send(Some(cached)) {
            tracing::warn!(subscription_id = payload.subscription_id.0, %error, "failed to update subscription cache");
        }
    }
}
