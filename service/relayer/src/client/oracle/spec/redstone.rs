use std::{sync::Arc, time::Duration};

use near_primitives::action::{Action, FunctionCallAction};
use near_sdk::{
    json_types::Base64VecU8,
    serde_json::{self, json},
};
use templar_common::oracle::redstone::FeedId;
use templar_redstone_bridge::{Bridge, BridgeError};
use tokio::sync::watch;

use crate::{
    app::args,
    cache::Cache,
    client::{near::Near, oracle::Handle},
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
        near: Near,
        cache: Cache,
        kill: watch::Sender<()>,
    ) -> Result<Handle<Self>, BridgeError> {
        Ok(Handle::new(
            Arc::new(Self::new(config, kill.clone())?),
            near,
            cache,
            kill,
        ))
    }
}

impl Spec for RedStoneSpec {
    type FeedId = FeedId;
    type Error = BridgeError;

    fn name() -> &'static str {
        "RedStone"
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self))]
    async fn update_actions(&self, feed_ids: &[Self::FeedId]) -> Result<Vec<Action>, Self::Error> {
        if feed_ids.is_empty() {
            return Ok(vec![]);
        }

        let payload_vec = self.bridge.fetch(feed_ids.to_vec()).await?;

        Ok(vec![FunctionCallAction {
            method_name: "write_prices".to_string(),
            #[allow(clippy::unwrap_used, reason = "This serialization is infallible")]
            args: serde_json::to_vec(&json!({
                "feed_ids": feed_ids,
                "payload": Base64VecU8(payload_vec),
            }))
            .unwrap(),
            gas: near_primitives::gas::Gas::from_gas(self.config.update_gas.as_gas()),
            deposit: self.config.update_deposit,
        }
        .into()])
    }
}
