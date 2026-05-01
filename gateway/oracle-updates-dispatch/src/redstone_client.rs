use std::path::Path;

use async_trait::async_trait;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_core::OraclePayloadSource;
use templar_redstone_bridge::Bridge;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RedStoneBridgeError {
    #[error("external service failed: {0}")]
    ExternalService(String),
}

pub type RedStoneResult<T> = Result<T, RedStoneBridgeError>;

#[derive(Debug, Clone)]
pub struct RedStoneBridgeClient {
    bridge: Bridge,
}

impl RedStoneBridgeClient {
    pub fn new(node_path: &Path) -> RedStoneResult<Self> {
        let (kill_tx, _kill_rx) = tokio::sync::watch::channel(());
        Ok(Self {
            bridge: Bridge::new(node_path, kill_tx)
                .map_err(|error| RedStoneBridgeError::ExternalService(error.to_string()))?,
        })
    }
}

#[async_trait]
impl OraclePayloadSource for RedStoneBridgeClient {
    type PriceId = FeedId;
    type Error = RedStoneBridgeError;

    async fn fetch_payload(&self, price_ids: &[Self::PriceId]) -> Result<Vec<u8>, Self::Error> {
        self.bridge
            .fetch(price_ids.to_vec())
            .await
            .map_err(|error| RedStoneBridgeError::ExternalService(error.to_string()))
    }
}
