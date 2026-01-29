use crate::{rpc::price_feed_exists, rpc::view, CliError, CliResult};
use near_jsonrpc_client::JsonRpcClient;
use near_sdk::AccountId;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use templar_common::number::Decimal;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::utils::Network;

pub struct PriceValidator {
    client: JsonRpcClient,
    http_client: Client,
}

#[derive(Deserialize)]
struct HermesEntry {
    id: String,
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
        let exists =
            match price_feed_exists(&self.client, oracle_contract_id.clone(), price_id).await {
                Ok(exists) => exists,
                Err(err) if is_prohibited_in_view(&err) => {
                    let underlying_oracle_id: AccountId = view(
                        &self.client,
                        oracle_contract_id.clone(),
                        "oracle_id",
                        json!({}),
                    )
                    .await
                    .map_err(|e| {
                        CliError::Oracle(format!(
                            "Failed to query oracle_id from {oracle_contract_id}: {e}"
                        ))
                    })?;

                    price_feed_exists(&self.client, underlying_oracle_id, price_id).await?
                }
                Err(err) => return Err(err),
            };

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
        let expected = price_id.to_string();
        let expected_hex = format!("0x{expected}");

        let mut url = Url::parse("https://hermes.pyth.network/v2/price_feeds")
            .map_err(|e| CliError::Oracle(format!("Failed to parse Hermes URL: {e}")))?;
        url.query_pairs_mut()
            .append_pair("query", token_symbol)
            .append_pair("asset_type", "crypto");

        let entries: Vec<HermesEntry> = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| {
                CliError::Oracle(format!(
                    "Hermes query failed for symbol '{token_symbol}': {e}"
                ))
            })?
            .error_for_status()
            .map_err(|e| {
                CliError::Oracle(format!(
                    "Hermes query failed for symbol '{token_symbol}': {e}"
                ))
            })?
            .json()
            .await
            .map_err(|e| {
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

fn is_prohibited_in_view(err: &CliError) -> bool {
    match err {
        CliError::NearRpc(message) => {
            message.contains("ProhibitedInView") || message.contains("promise_batch_create")
        }
        _ => false,
    }
}

/// # Errors
pub async fn fetch_oracle_price(token_symbol: &str) -> CliResult<Decimal> {
    #[derive(Deserialize)]
    struct HermesLatestResponse {
        parsed: Vec<HermesParsedPrice>,
    }

    #[derive(Deserialize)]
    struct HermesParsedPrice {
        price: HermesPrice,
    }

    #[derive(Deserialize)]
    struct HermesPrice {
        price: String,
        expo: i32,
    }

    let http_client = Client::new();

    let mut url = Url::parse("https://hermes.pyth.network/v2/price_feeds")
        .map_err(|e| CliError::Oracle(format!("Failed to parse Hermes URL: {e}")))?;

    url.query_pairs_mut()
        .append_pair("query", token_symbol)
        .append_pair("asset_type", "crypto");

    let entries: Vec<HermesEntry> = http_client
        .get(url)
        .send()
        .await
        .map_err(|e| {
            CliError::Oracle(format!(
                "Hermes query failed for symbol '{token_symbol}': {e}"
            ))
        })?
        .error_for_status()
        .map_err(|e| {
            CliError::Oracle(format!(
                "Hermes query failed for symbol '{token_symbol}': {e}"
            ))
        })?
        .json()
        .await
        .map_err(|e| {
            CliError::Oracle(format!(
                "Failed to parse Hermes response for symbol '{token_symbol}': {e}"
            ))
        })?;

    let entry = entries.first().ok_or_else(|| {
        CliError::Oracle(format!(
            "No Hermes price feeds found for symbol '{token_symbol}'"
        ))
    })?;

    let mut latest_url = Url::parse("https://hermes.pyth.network/v2/updates/price/latest")
        .map_err(|e| CliError::Oracle(format!("Failed to parse Hermes updates URL: {e}")))?;
    latest_url.query_pairs_mut().append_pair("ids[]", &entry.id);

    let latest: HermesLatestResponse = http_client
        .get(latest_url)
        .send()
        .await
        .map_err(|e| {
            CliError::Oracle(format!(
                "Hermes updates query failed for symbol '{token_symbol}': {e}"
            ))
        })?
        .error_for_status()
        .map_err(|e| {
            CliError::Oracle(format!(
                "Hermes updates query failed for symbol '{token_symbol}': {e}"
            ))
        })?
        .json()
        .await
        .map_err(|e| {
            CliError::Oracle(format!(
                "Failed to parse Hermes updates response for symbol '{token_symbol}': {e}"
            ))
        })?;
    let parsed = latest.parsed.first().ok_or_else(|| {
        CliError::Oracle(format!(
            "Hermes updates response missing price data for '{token_symbol}'"
        ))
    })?;
    let price = parsed.price.price.parse::<i128>().map_err(|e| {
        CliError::Oracle(format!(
            "Failed to parse Hermes price for symbol '{token_symbol}': {e}"
        ))
    })?;
    if price <= 0 {
        return Err(CliError::Oracle(format!(
            "Hermes price for symbol '{token_symbol}' is non-positive"
        )));
    }
    let abs = u128::try_from(price).map_err(|_| {
        CliError::Oracle(format!(
            "Hermes price for symbol '{token_symbol}' is out of range"
        ))
    })?;
    let base = Decimal::from(abs);
    let scale = Decimal::from_u32(10).pow(parsed.price.expo);
    Ok(base * scale)
}
