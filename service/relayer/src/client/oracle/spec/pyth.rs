use std::{sync::Arc, time::Duration};

use near_primitives::action::{Action, FunctionCallAction};
use near_sdk::serde::Deserialize;
use templar_common::oracle::pyth;
use tokio::sync::watch;

use crate::{
    app::args,
    cache::Cache,
    client::{near::Near, oracle::Handle},
};

use super::Spec;

#[derive(Debug, Clone)]
pub struct PythSpec {
    http: reqwest::Client,
    config: args::PythConfig,
}

impl PythSpec {
    pub fn new(config: args::PythConfig) -> Self {
        Self {
            #[allow(
                clippy::unwrap_used,
                reason = "Only panics if TLS backend fails to initialize, which is both unlikely and unrecoverable."
            )]
            http: reqwest::Client::builder()
                .timeout(config.timeout)
                .build()
                .unwrap(),
            config,
        }
    }

    pub fn handle(
        config: args::PythConfig,
        near: Near,
        cache: Cache,
        kill: watch::Sender<()>,
    ) -> Handle<Self> {
        Handle::new(Arc::new(Self::new(config)), near, cache, kill)
    }

    /// Fetch just the update payload for a set of price IDs.
    ///
    /// # Errors
    ///
    /// - [`reqwest::Error`]
    /// - Response deserialization.
    async fn latest_vaa(
        &self,
        price_ids: &[pyth::PriceIdentifier],
    ) -> Result<Vec<u8>, reqwest::Error> {
        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct ResponseBody {
            binary: Binary,
        }

        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct Binary {
            data: [Data; 1],
        }

        #[derive(Deserialize)]
        #[serde(crate = "near_sdk::serde")]
        struct Data(#[serde(deserialize_with = "hex::deserialize")] Vec<u8>);

        let mut request = self.http.get(format!(
            "{}/v2/updates/price/latest",
            self.config.hermes_url
        ));

        for id in price_ids {
            request = request.query(&[("ids[]", id)]);
        }

        let response = request.send().await?.error_for_status()?;

        let body = response.json::<ResponseBody>().await?;
        let [vaa] = body.binary.data;
        Ok(vaa.0)
    }
}

impl Spec for PythSpec {
    type FeedId = pyth::PriceIdentifier;
    type Error = reqwest::Error;

    fn name() -> &'static str {
        "pyth"
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self))]
    async fn update_actions(&self, feed_ids: &[Self::FeedId]) -> Result<Vec<Action>, Self::Error> {
        if feed_ids.is_empty() {
            return Ok(vec![]);
        }

        let vaa = self.latest_vaa(feed_ids).await?;
        let args = format!(r#"{{"data":"{}"}}"#, hex::encode(vaa)).into_bytes();
        Ok(vec![FunctionCallAction {
            method_name: "update_price_feeds".to_string(),
            args,
            gas: self.config.update_gas.as_gas(),
            deposit: self.config.update_deposit.as_yoctonear(),
        }
        .into()])
    }
}
