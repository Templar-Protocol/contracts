use crate::{rpc::price_feed_exists, CliError, CliResult};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use reqwest::{Client, Url};
use serde::Deserialize;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::utils::Network;

pub struct PriceValidator {
    client: JsonRpcClient,
    http_client: Client,
}

impl PriceValidator {
    pub fn new(network: Network) -> Self {
        let rpc_url = network.rpc_url();
        Self {
            client: JsonRpcClient::connect(rpc_url),
            http_client: Client::new(),
        }
    }

    /// Validate that a price feed exists on the oracle contract
    /// # Errors
    pub async fn validate_price_feed(
        &self,
        oracle_contract_id: AccountId,
        price_id: &PriceIdentifier,
    ) -> CliResult {
        let exists = price_feed_exists(&self.client, oracle_contract_id.clone(), price_id).await?;

        let price_hash = format!("0x{price_id}");

        if !exists {
            return Err(CliError::Oracle(format!(
                "Price feed {price_hash} does not exist on oracle contract {oracle_contract_id}"
            )));
        }

        Ok(())
    }

    /// Validate that the price ID matches Hermes data for the token symbol.
    /// # Errors
    pub async fn validate_price_feed_matches_symbol(
        &self,
        token_symbol: &str,
        price_id: &PriceIdentifier,
    ) -> CliResult {
        #[derive(Deserialize)]
        struct HermesEntry {
            id: String,
        }

        let expected = price_id.to_string();
        let expected_hex = format!("0x{expected}");

        let mut url = Url::parse("https://hermes.pyth.network/v2/price_feeds")
            .map_err(|e| CliError::Oracle(format!("Failed to parse Hermes URL: {e}")))?;
        url.query_pairs_mut()
            .append_pair("query", token_symbol)
            .append_pair("asset_type", "crypto");

        let response = self.http_client.get(url).send().await.map_err(|e| {
            CliError::Oracle(format!(
                "Hermes query failed for symbol '{token_symbol}': {e}"
            ))
        })?;
        let response = response.error_for_status().map_err(|e| {
            CliError::Oracle(format!(
                "Hermes query failed for symbol '{token_symbol}': {e}"
            ))
        })?;
        let entries: Vec<HermesEntry> = response.json().await.map_err(|e| {
            CliError::Oracle(format!(
                "Failed to parse Hermes response for symbol '{token_symbol}': {e}"
            ))
        })?;
        let matches = entries.iter().any(|entry| {
            let trimmed = entry.id.trim().trim_start_matches("0x");
            trimmed.eq_ignore_ascii_case(expected.as_str())
        });

        if matches {
            Ok(())
        } else {
            Err(CliError::Oracle(format!(
                "Hermes price feed mismatch for symbol '{token_symbol}': expected {expected_hex}"
            )))
        }
    }
}
