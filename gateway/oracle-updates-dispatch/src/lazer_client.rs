use std::{collections::BTreeSet, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use templar_gateway_core::OraclePayloadSource;
use thiserror::Error;
use tokio::{sync::RwLock, task::JoinHandle, time::Instant};
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

use crate::lazer_wire::{decode_stream_message, subscription_frame};

const DEFAULT_CHANNEL: &str = "fixed_rate@200ms";
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
const MAX_LAZER_STREAM_MESSAGE_BYTES: usize = 1_048_576;

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct LazerSourceConfig {
    ws_url: Url,
    api_token: String,
    price_feed_ids: BTreeSet<u32>,
    channel: String,
    max_payload_age: Duration,
}

#[derive(Debug, Clone)]
pub struct LazerSubscriptionConfig {
    pub price_feed_ids: Vec<u32>,
    pub channel: Option<String>,
    pub max_payload_age: Duration,
}

impl fmt::Debug for LazerSourceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazerSourceConfig")
            .field("ws_url", &self.ws_url)
            .field("api_token", &"<redacted>")
            .field("price_feed_ids", &self.price_feed_ids)
            .field("channel", &self.channel)
            .field("max_payload_age", &self.max_payload_age)
            .finish()
    }
}

impl LazerSourceConfig {
    pub fn new(
        ws_url: Url,
        api_token: String,
        subscription: LazerSubscriptionConfig,
    ) -> LazerResult<Self> {
        if ws_url.scheme() != "wss" {
            return Err(LazerClientError::InsecureWebSocketUrl);
        }
        if api_token.trim().is_empty() {
            return Err(LazerClientError::EmptyApiToken);
        }
        let price_feed_ids = subscription
            .price_feed_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        if price_feed_ids.is_empty() {
            return Err(LazerClientError::EmptySubscription);
        }
        Ok(Self {
            ws_url,
            api_token,
            price_feed_ids,
            channel: subscription
                .channel
                .unwrap_or_else(|| DEFAULT_CHANNEL.to_owned()),
            max_payload_age: subscription.max_payload_age,
        })
    }

    pub(crate) fn price_feed_ids(&self) -> &BTreeSet<u32> {
        &self.price_feed_ids
    }

    pub(crate) fn channel(&self) -> &str {
        &self.channel
    }
}

#[derive(Debug, Error)]
pub enum LazerClientError {
    #[error("Pyth Lazer websocket URL must use wss://")]
    InsecureWebSocketUrl,
    #[error("Pyth Lazer API token must not be empty")]
    EmptyApiToken,
    #[error("Pyth Lazer subscription must include at least one price feed id")]
    EmptySubscription,
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
    #[error("Pyth Lazer cache does not cover requested feed id {0}")]
    FeedNotCovered(u32),
    #[error("Pyth Lazer cached payload is stale")]
    StalePayload,
}

pub type LazerResult<T> = Result<T, LazerClientError>;

#[derive(Debug, Clone)]
pub struct LazerPayloadSource {
    inner: Arc<LazerPayloadSourceInner>,
    _task: Arc<TaskGuard>,
}

#[derive(Debug)]
struct LazerPayloadSourceInner {
    config: LazerSourceConfig,
    cache: RwLock<Option<CachedPayload>>,
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

impl LazerPayloadSource {
    pub fn spawn(config: LazerSourceConfig) -> Self {
        let inner = Arc::new(LazerPayloadSourceInner {
            config,
            cache: RwLock::new(None),
        });
        let task_inner = Arc::clone(&inner);
        let handle = tokio::spawn(async move { task_inner.run().await });
        let task = Arc::new(TaskGuard {
            task: std::sync::Mutex::new(None),
        });
        if let Ok(mut slot) = task.task.lock() {
            *slot = Some(handle);
        }
        Self { inner, _task: task }
    }

    #[cfg(test)]
    fn from_cached(config: LazerSourceConfig, payload: Option<CachedPayload>) -> Self {
        Self {
            inner: Arc::new(LazerPayloadSourceInner {
                config,
                cache: RwLock::new(payload),
            }),
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
        for price_id in price_ids {
            if !self.inner.config.price_feed_ids.contains(price_id) {
                return Err(LazerClientError::FeedNotCovered(*price_id));
            }
        }

        let cache = self.inner.cache.read().await;
        let cached = cache.as_ref().ok_or(LazerClientError::CacheMiss)?;
        for price_id in price_ids {
            if !cached.feed_ids.contains(price_id) {
                return Err(LazerClientError::FeedNotCovered(*price_id));
            }
        }
        if cached.received_at.elapsed() > self.inner.config.max_payload_age {
            return Err(LazerClientError::StalePayload);
        }
        Ok(cached.payload.clone())
    }
}

impl LazerPayloadSourceInner {
    async fn run(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.connect_and_stream().await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    tracing::warn!(%error, "Pyth Lazer stream disconnected");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_RECONNECT_BACKOFF);
                }
            }
        }
    }

    async fn connect_and_stream(&self) -> LazerResult<()> {
        let mut request = self
            .config
            .ws_url
            .as_str()
            .into_client_request()
            .map_err(|error| LazerClientError::Request(error.to_string()))?;
        let authorization = HeaderValue::from_str(&format!("Bearer {}", self.config.api_token))
            .map_err(|error| LazerClientError::Request(error.to_string()))?;
        request.headers_mut().insert(AUTHORIZATION, authorization);

        let (mut stream, _) = connect_async_with_config(
            request,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(MAX_LAZER_STREAM_MESSAGE_BYTES))
                    .max_frame_size(Some(MAX_LAZER_STREAM_MESSAGE_BYTES)),
            ),
            false,
        )
        .await
        .map_err(|error| LazerClientError::Request(error.to_string()))?;
        stream
            .send(Message::Text(subscription_frame(&self.config)?.into()))
            .await
            .map_err(|error| LazerClientError::Request(error.to_string()))?;

        while let Some(message) = stream.next().await {
            let message = message.map_err(|error| LazerClientError::Request(error.to_string()))?;
            let Message::Text(text) = message else {
                continue;
            };
            match decode_stream_message(text.as_ref()) {
                Ok(Some(payload)) => {
                    *self.cache.write().await = Some(CachedPayload {
                        payload: payload.bytes,
                        feed_ids: payload.feed_ids,
                        received_at: Instant::now(),
                    });
                }
                Ok(None) => tracing::debug!("ignored non-update Pyth Lazer stream message"),
                Err(error) => tracing::warn!(%error, "ignored invalid Pyth Lazer stream message"),
            }
        }
        Err(LazerClientError::Request("websocket closed".to_owned()))
    }
}
