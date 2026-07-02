use std::{sync::Arc, time::Duration};

use near_sdk::{serde::Deserialize, AccountId};
use templar_common::oracle::pyth;
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::pyth as pyth_spec;
use templar_gateway_types::{Base64Bytes, CryptoHash};
use tokio::sync::watch;

use crate::{
    app::args,
    client::oracle::{succeeded_tx_hash, Handle, UpdateError},
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
        gateway: SigningClient,
        kill: watch::Sender<()>,
    ) -> Handle<Self> {
        Handle::new(Arc::new(Self::new(config)), gateway, kill)
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

    fn name() -> &'static str {
        "pyth"
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

        let vaa = self
            .latest_vaa(feed_ids)
            .await
            .map_err(|e| UpdateError::Fetch(Box::new(e)))?;

        let result = gateway
            .execute(pyth_spec::UpdatePriceFeeds {
                oracle_id,
                data: Base64Bytes(vaa),
            })
            .await?;

        succeeded_tx_hash(result)
    }
}
