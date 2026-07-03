use std::{collections::BTreeMap, collections::BTreeSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pyth_lazer_protocol::{
    api::{Channel, SubscriptionId},
    time::FixedRate,
};
use templar_gateway_core::{OraclePayloadSource, RedactedString};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot, watch, RwLock},
    task::JoinHandle,
    time::Instant,
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
    decode_stream_message, subscription_frame_for_feeds, MAX_STREAM_JSON_MESSAGE_BYTES,
};

const DEFAULT_CHANNEL: Channel = Channel::FixedRate(FixedRate::RATE_200_MS);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);

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
    pub channel: Option<String>,
    pub max_payload_age: Duration,
}

impl LazerSourceConfig {
    pub fn new(ws_url: Url, api_token: RedactedString, subscription: LazerSubscriptionConfig) -> LazerResult<Self> {
        if ws_url.scheme() != "wss" {
            return Err(LazerClientError::InsecureWebSocketUrl);
        }
        if api_token.trim().is_empty() {
            return Err(LazerClientError::EmptyApiToken);
        }
        if subscription.max_payload_age.is_zero() {
            return Err(LazerClientError::InvalidMaxPayloadAge);
        }
        let channel = parse_channel(subscription.channel)?;
        Ok(Self {
            ws_url,
            api_token,
            channel,
            max_payload_age: subscription.max_payload_age,
        })
    }

    pub(crate) fn channel(&self) -> Channel {
        self.channel
    }
}

fn parse_channel(channel: Option<String>) -> LazerResult<Channel> {
    let Some(channel) = channel else {
        return Ok(DEFAULT_CHANNEL);
    };
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
    #[error("unsupported Pyth Lazer solana payload encoding: {0}")]
    UnsupportedEncoding(String),
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

#[derive(Debug, Default)]
struct SubscriptionState {
    active: BTreeMap<SubscriptionId, SubscriptionInfo>,
    next_id: u64,
}

#[derive(Debug, Clone)]
struct SubscriptionInfo {
    feed_ids: BTreeSet<u32>,
    cache: watch::Receiver<Option<CachedPayload>>,
    cache_tx: Arc<watch::Sender<Option<CachedPayload>>>,
}

#[derive(Debug)]
struct TaskGuard {
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone)]
struct CachedPayload {
    payload: Vec<u8>,
    feed_ids: BTreeSet<u32>,
    received_at: Instant,
}

struct SubscribeRequest {
    feed_ids: BTreeSet<u32>,
    response_tx: oneshot::Sender<LazerResult<SubscriptionId>>,
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
        let task = Arc::new(TaskGuard {
            task: std::sync::Mutex::new(None),
        });
        if let Ok(mut slot) = task.task.lock() {
            *slot = Some(handle);
        }
        Self {
            inner,
            subscribe_tx,
            _task: task,
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
        subscriptions.next_id = 2;
        Self {
            inner: Arc::new(LazerPayloadSourceInner {
                config,
                subscriptions: RwLock::new(subscriptions),
            }),
            subscribe_tx,
            _task: Arc::new(TaskGuard {
                task: std::sync::Mutex::new(None),
            }),
        }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if let Ok(mut task) = self.task.lock() {
            if let Some(task) = task.take() {
                task.abort();
            }
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

        let subscriptions = self.inner.subscriptions.read().await;
        let subscription = subscriptions
            .active
            .get(&subscription_id)
            .ok_or_else(|| LazerClientError::CacheMiss)?;

        let cache_rx = subscription.cache.clone();
        drop(subscriptions);

        let cached = cache_rx
            .borrow()
            .clone()
            .ok_or(LazerClientError::CacheMiss)?;

        if cached.received_at.elapsed() > self.inner.config.max_payload_age {
            return Err(LazerClientError::StalePayload);
        }

        Ok(cached.payload)
    }
}

impl LazerPayloadSource {
    async fn ensure_subscription(&self, feed_ids: BTreeSet<u32>) -> LazerResult<SubscriptionId> {
        {
            let subscriptions = self.inner.subscriptions.read().await;
            for (id, info) in &subscriptions.active {
                if feed_ids.is_subset(&info.feed_ids) {
                    return Ok(*id);
                }
            }
        }

        let (response_tx, response_rx) = oneshot::channel();
        self.subscribe_tx
            .send(SubscribeRequest {
                feed_ids,
                response_tx,
            })
            .await
            .map_err(|_| LazerClientError::Request("subscription channel closed".to_owned()))?;

        response_rx
            .await
            .map_err(|_| LazerClientError::Request("subscription response channel closed".to_owned()))?
    }
}

impl LazerPayloadSourceInner {
    async fn run(self: Arc<Self>, mut subscribe_rx: mpsc::Receiver<SubscribeRequest>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if let Err(error) = self.connect_and_stream(&mut subscribe_rx, &mut backoff).await {
                tracing::warn!(%error, "Pyth Lazer stream disconnected");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_RECONNECT_BACKOFF);
            }
        }
    }

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
        let authorization = HeaderValue::from_str(&format!("Bearer {}", self.config.api_token.as_ref()))
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

        loop {
            tokio::select! {
                message = stream.next() => {
                    let Some(message) = message else {
                        break;
                    };
                    let message = message.map_err(|error| LazerClientError::Request(error.to_string()))?;
                    let Message::Text(text) = message else {
                        continue;
                    };
                    match decode_stream_message(text.as_ref()) {
                        Ok(Some(payload)) => {
                            *backoff = Duration::from_secs(1);
                            self.update_subscription_cache(payload).await;
                        }
                        Ok(None) => tracing::debug!("ignored non-update Pyth Lazer stream message"),
                        Err(error) => tracing::warn!(%error, "ignored invalid Pyth Lazer stream message"),
                    }
                }
                req = subscribe_rx.recv() => {
                    let Some(req) = req else {
                        break;
                    };
                    self.handle_subscribe_request(&mut stream, req).await?;
                }
            }
        }
        Err(LazerClientError::Request("websocket closed".to_owned()))
    }

    async fn handle_subscribe_request(
        &self,
        stream: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        req: SubscribeRequest,
    ) -> LazerResult<()> {
        let subscription_id = {
            let mut subscriptions = self.subscriptions.write().await;
            let id = SubscriptionId(subscriptions.next_id);
            subscriptions.next_id += 1;
            let (cache_tx, cache_rx) = watch::channel(None);
            subscriptions.active.insert(
                id,
                SubscriptionInfo {
                    feed_ids: req.feed_ids.clone(),
                    cache: cache_rx,
                    cache_tx: Arc::new(cache_tx),
                },
            );
            id
        };

        let frame = subscription_frame_for_feeds(&self.config, subscription_id, req.feed_ids.clone())?;
        stream
            .send(Message::Text(frame.into()))
            .await
            .map_err(|error| LazerClientError::Request(error.to_string()))?;

        let _ = req.response_tx.send(Ok(subscription_id));
        Ok(())
    }

    async fn update_subscription_cache(&self, payload: crate::lazer_wire::DecodedLazerPayload) {
        let subscriptions = self.subscriptions.read().await;
        for (id, info) in &subscriptions.active {
            if payload.feed_ids.is_subset(&info.feed_ids) {
                let cached = CachedPayload {
                    payload: payload.bytes.clone(),
                    feed_ids: payload.feed_ids.clone(),
                    received_at: Instant::now(),
                };
                if let Err(error) = info.cache_tx.send(Some(cached)) {
                    tracing::warn!(subscription_id = id.0, %error, "failed to update subscription cache");
                }
                break;
            }
        }
    }
}
