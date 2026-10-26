use std::path::PathBuf;

use async_trait::async_trait;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_core::{OraclePayloadSource, RedactedString};
use templar_redstone_bridge::Bridge;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RedStoneBridgeError {
    #[error("external service failed: {0}")]
    ExternalService(String),
    #[error("RedStone API key must not be empty")]
    EmptyApiKey,
}

pub type RedStoneResult<T> = Result<T, RedStoneBridgeError>;

#[derive(Debug, Clone)]
pub struct RedStoneSourceConfig {
    node_path: PathBuf,
    api_key: RedactedString,
}

impl RedStoneSourceConfig {
    /// `node_path` runs the RedStone bridge; `api_key` authenticates against
    /// RedStone's oracle gateway.
    pub fn new(node_path: PathBuf, api_key: RedactedString) -> RedStoneResult<Self> {
        if api_key.trim().is_empty() {
            return Err(RedStoneBridgeError::EmptyApiKey);
        }
        Ok(Self { node_path, api_key })
    }
}

#[derive(Debug, Clone)]
pub struct RedStoneBridgeClient {
    bridge: Bridge,
}

impl RedStoneBridgeClient {
    pub fn new(config: &RedStoneSourceConfig) -> RedStoneResult<Self> {
        let (kill_tx, _kill_rx) = tokio::sync::watch::channel(());
        Ok(Self {
            bridge: Bridge::new(&config.node_path, &config.api_key, kill_tx)
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::empty("")]
    #[case::blank("  ")]
    fn config_rejects_blank_api_key(#[case] api_key: &str) {
        assert!(matches!(
            RedStoneSourceConfig::new("node".into(), api_key.into()),
            Err(RedStoneBridgeError::EmptyApiKey)
        ));
    }

    #[test]
    fn config_redacts_api_key() {
        let config = RedStoneSourceConfig::new("node".into(), "raw-secret".into()).unwrap();
        assert!(!format!("{config:?}").contains("raw-secret"));
    }
}
