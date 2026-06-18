use anyhow::{anyhow, bail, Result};
use serde::de::DeserializeOwned;
use templar_gateway_types::MethodSpec;

#[derive(Debug, Clone)]
pub struct TestController {
    client: reqwest::Client,
    rpc_url: String,
}

impl TestController {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url: rpc_url.into(),
        }
    }

    pub fn request_url(&self) -> &str {
        &self.rpc_url
    }

    pub async fn request<Spec>(&self, params: &Spec::Input) -> Result<Spec::Output>
    where
        Spec: MethodSpec,
        Spec::Input: serde::Serialize,
        Spec::Output: DeserializeOwned,
    {
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": Spec::RPC_METHOD,
                "params": params,
                "id": 1,
            }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            bail!("gateway http error {status}: {body}");
        }

        let value: serde_json::Value = serde_json::from_str(&body)?;

        if let Some(error) = value.get("error") {
            bail!("gateway rpc error: {error}");
        }

        let result = value
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("missing rpc result in response: {value}"))?;

        Ok(serde_json::from_value(result)?)
    }
}
