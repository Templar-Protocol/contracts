use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use near_sdk::serde::Deserialize;
use templar_common::oracle::pyth::PriceIdentifier;
use tokio::{sync::Mutex, time::Instant};

use crate::app::args;

#[derive(Debug, Clone)]
pub struct Pyth {
    http: reqwest::Client,
    args: args::Pyth,
    last_updated: Arc<Mutex<HashMap<PriceIdentifier, Instant>>>,
}

impl Pyth {
    pub fn new(args: args::Pyth) -> Self {
        Self {
            http: reqwest::Client::new(),
            args,
            last_updated: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn needs_update(
        &self,
        price_ids: impl IntoIterator<Item = &PriceIdentifier>,
    ) -> HashSet<PriceIdentifier> {
        let mut set = HashSet::new();
        let last_updated = self.last_updated.lock().await;
        for price_id in price_ids {
            if last_updated
                .get(price_id)
                .is_none_or(|i| i.elapsed() >= self.args.refresh)
            {
                set.insert(*price_id);
            }
        }
        set
    }

    pub async fn mark_update(&mut self, price_ids: impl IntoIterator<Item = &PriceIdentifier>) {
        let now = Instant::now();
        let mut last_updated = self.last_updated.lock().await;
        for price_id in price_ids {
            last_updated.insert(*price_id, now);
        }
    }

    /// Fetch just the update payload for a set of price IDs.
    ///
    /// # Errors
    ///
    /// - [`reqwest::Error`]
    /// - Response deserialization.
    #[tracing::instrument(skip_all)]
    pub async fn get_latest_price_updates_vaa(
        &self,
        price_ids: impl IntoIterator<Item = &PriceIdentifier>,
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

        let mut request = self
            .http
            .get(format!("{}/v2/updates/price/latest", self.args.hermes_url));

        for id in price_ids {
            request = request.query(&[("ids[]", id)]);
        }

        let response = request.send().await?.error_for_status()?;

        let body = response.json::<ResponseBody>().await?;
        let [vaa] = body.binary.data;
        Ok(vaa.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;
    use test_utils::pyth_price_id;

    #[tokio::test]
    async fn fetch() {
        let c = Pyth::new(args::Pyth {
            hermes_url: "https://hermes.pyth.network".into(),
            refresh: Duration::from_secs(25),
            oracle_id: "pyth-oracle.near".parse().unwrap(),
            push_tgas: 300,
            push_deposit: near_sdk::NearToken::from_near(1).saturating_div(10),
        });

        let vaa = c
            .get_latest_price_updates_vaa(&[pyth_price_id::stable::CRYPTO_BTC_USD])
            .await
            .unwrap();

        eprintln!("VAA: {}", hex::encode(vaa));
    }
}
