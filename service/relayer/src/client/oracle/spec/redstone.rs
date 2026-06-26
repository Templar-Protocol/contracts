use std::{sync::Arc, time::Duration};

use near_sdk::AccountId;
use templar_common::oracle::redstone::FeedId;
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::redstone as redstone_spec;
use templar_gateway_types::{Base64Bytes, CryptoHash};
use templar_redstone_bridge::{Bridge, BridgeError};
use tokio::sync::watch;

use crate::{
    app::args,
    client::oracle::{succeeded_tx_hash, Handle, UpdateError},
};

use super::Spec;

#[derive(Debug, Clone)]
pub struct RedStoneSpec {
    bridge: Bridge,
    config: args::RedStoneConfig,
}

impl RedStoneSpec {
    /// # Errors
    ///
    /// Returns an error if the embedded JS bridge bundle is unavailable.
    pub fn new(config: args::RedStoneConfig, kill: watch::Sender<()>) -> Result<Self, BridgeError> {
        let bridge = Bridge::new(&config.node_path, kill)?;
        Ok(Self { bridge, config })
    }

    pub fn handle(
        config: args::RedStoneConfig,
        gateway: SigningClient,
        kill: watch::Sender<()>,
    ) -> Result<Handle<Self>, BridgeError> {
        Ok(Handle::new(
            Arc::new(Self::new(config, kill.clone())?),
            gateway,
            kill,
        ))
    }
}

impl Spec for RedStoneSpec {
    type FeedId = FeedId;

    fn name() -> &'static str {
        "RedStone"
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self, gateway))]
    async fn execute_update(
        &self,
        gateway: &SigningClient,
        oracle_id: AccountId,
        feed_ids: &[Self::FeedId],
    ) -> Result<Option<CryptoHash>, UpdateError> {
        if feed_ids.is_empty() {
            return Ok(None);
        }

        let payload = self
            .bridge
            .fetch(feed_ids.to_vec())
            .await
            .map_err(|e| UpdateError::Fetch(Box::new(e)))?;

        let result = gateway
            .execute(redstone_spec::WritePrices {
                oracle_id,
                feed_ids: feed_ids.to_vec(),
                payload: Base64Bytes(payload),
            })
            .await?;

        succeeded_tx_hash(result)
    }
}
