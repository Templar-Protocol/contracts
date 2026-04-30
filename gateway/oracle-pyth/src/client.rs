use async_trait::async_trait;
use serde::Deserialize;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_core::OraclePayloadSource;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum PythClientError {
    #[error("http request failed: {0}")]
    HttpRequest(String),
}

pub type PythResult<T> = Result<T, PythClientError>;

#[derive(Debug, Clone)]
pub struct PythHttpClient {
    http: reqwest::Client,
    hermes_url: Url,
}

impl PythHttpClient {
    pub fn new(hermes_url: Url) -> Self {
        Self {
            http: reqwest::Client::new(),
            hermes_url,
        }
    }

    pub async fn fetch_latest_vaa(&self, price_ids: &[PriceIdentifier]) -> PythResult<Vec<u8>> {
        self.fetch_payload(price_ids).await
    }
}

#[async_trait]
impl OraclePayloadSource for PythHttpClient {
    type PriceId = PriceIdentifier;
    type Error = PythClientError;

    async fn fetch_payload(&self, price_ids: &[Self::PriceId]) -> Result<Vec<u8>, Self::Error> {
        #[derive(Deserialize)]
        struct ResponseBody {
            binary: Binary,
        }

        #[derive(Deserialize)]
        struct Binary {
            data: [Data; 1],
        }

        #[derive(Deserialize)]
        struct Data(#[serde(deserialize_with = "hex::deserialize")] Vec<u8>);

        let mut request = self.http.get(format!(
            "{}/v2/updates/price/latest",
            self.hermes_url.as_str().trim_end_matches('/'),
        ));

        for price_id in price_ids {
            request = request.query(&[("ids[]", price_id)]);
        }

        let response = request
            .send()
            .await
            .map_err(|error| PythClientError::HttpRequest(error.to_string()))?
            .error_for_status()
            .map_err(|error| PythClientError::HttpRequest(error.to_string()))?;

        let body = response
            .json::<ResponseBody>()
            .await
            .map_err(|error| PythClientError::HttpRequest(error.to_string()))?;
        let [vaa] = body.binary.data;
        Ok(vaa.0)
    }
}
