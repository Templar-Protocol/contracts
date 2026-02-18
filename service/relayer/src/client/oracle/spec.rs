use std::{sync::Arc, time::Duration};

use near_primitives::transaction::SignedTransaction;
use near_sdk::serde::Deserialize;
use templar_common::oracle::pyth;
use tokio::sync::watch;

use crate::{app::args, cache::Cache, client::near::Near};

pub trait Spec: Send + Sync + 'static {
    type PriceIdentifier: std::hash::Hash + std::fmt::Debug + std::cmp::Eq + Clone + Send + Sync;
    type Error: std::error::Error + 'static + Send + Sync;

    fn name() -> &'static str;
    fn refresh(&self) -> Duration;
    fn update_transaction(
        &self,
        price_ids: &[Self::PriceIdentifier],
    ) -> impl std::future::Future<Output = Result<SignedTransaction, Self::Error>> + Send + Sync;
}

#[derive(Debug, Clone)]
pub struct PythSpec {
    http: reqwest::Client,
    config: args::Pyth,
    near: Near,
    cache: Cache,
}

impl PythSpec {
    pub fn new(config: args::Pyth, near: Near, cache: Cache) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            near,
            cache,
        }
    }

    pub fn handle(
        config: args::Pyth,
        near: Near,
        cache: Cache,
        kill: watch::Sender<()>,
    ) -> super::Handle<Self> {
        let spec = Arc::new(Self::new(config, near.clone(), cache));
        super::Handle::new(spec, near, kill)
    }

    /// Fetch just the update payload for a set of price IDs.
    ///
    /// # Errors
    ///
    /// - [`reqwest::Error`]
    /// - Response deserialization.
    async fn get_latest_price_updates_vaa(
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
    type PriceIdentifier = pyth::PriceIdentifier;
    type Error = reqwest::Error;

    fn name() -> &'static str {
        "pyth"
    }

    fn refresh(&self) -> Duration {
        self.config.refresh
    }

    #[tracing::instrument(skip(self))]
    async fn update_transaction(
        &self,
        price_ids: &[Self::PriceIdentifier],
    ) -> Result<SignedTransaction, Self::Error> {
        let vaa = self.get_latest_price_updates_vaa(price_ids).await?;
        let tx = self
            .near
            .construct_pyth_update_transaction(
                &self.cache,
                self.config.oracle_id.clone(),
                vaa,
                self.config.update_gas,
                self.config.update_deposit,
            )
            .await;
        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use near_jsonrpc_client::JsonRpcClient;
    use near_sdk::NearToken;
    use templar_common::oracle::pyth::PriceIdentifier;

    use crate::app::args;

    use super::*;

    #[tokio::test]
    async fn fetch_vaa() {
        let pyth_args = args::Pyth {
            hermes_url: "https://hermes-beta.pyth.network".to_string(),
            refresh: Duration::from_secs(25),
            oracle_id: "pyth-oracle.testnet".parse().unwrap(),
            update_gas: near_sdk::Gas::from_tgas(300),
            update_deposit: NearToken::from_near(1).saturating_div(100),
        };
        let near = Near::new(
            JsonRpcClient::connect("https://test.rpc.fastnear.com"),
            "irrelevant".parse().unwrap(),
            vec![],
        );

        let cache_args = args::Cache {
            gas_price_refresh: Duration::from_secs(600),
            nonce_refresh: Duration::from_secs(60),
        };

        let kill = watch::Sender::default();

        let cache = Cache::new(near.clone(), cache_args, kill.clone());

        let handle = PythSpec::new(pyth_args.clone(), near.clone(), cache.clone());

        let price_id = PriceIdentifier(
            hex::decode("f9c0172ba10dfa4d19088d94f5bf61d3b54d5bd7483a322a982e1373ee8ea31b")
                .unwrap()
                .try_into()
                .unwrap(),
        );

        let vaa = handle
            .get_latest_price_updates_vaa(&[price_id])
            .await
            .unwrap();

        assert_ne!(vaa, Vec::<u8>::new());
    }
}
