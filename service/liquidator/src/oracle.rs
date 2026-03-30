//! Oracle price fetching module.
//!
//! Handles fetching prices from various oracle types including:
//! - Pyth oracles (via Hermes HTTP API)
//! - RedStone oracles (via gateway HTTP API)
//! - LST oracles with price transformers
//! - Proxy oracles with off-chain aggregation

use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{serde_json::json, AccountId};
use std::collections::HashMap;
use templar_common::{
    number::Decimal,
    oracle::{
        price_transformer::PriceTransformer,
        proxy::{Proxy, Source},
        pyth::{self, OracleResponse, PriceIdentifier},
        redstone, OracleRequest,
    },
    time::Nanoseconds,
};

use crate::{
    rpc::{view, RpcError},
    LiquidatorError, LiquidatorResult,
};

// ── Hermes (Pyth) gateway types ──────────────────────────────────────────────

/// Parsed response from Pyth Hermes `/v2/updates/price/latest?parsed=true`.
#[derive(serde::Deserialize)]
struct HermesResponse {
    parsed: Option<Vec<HermesParsedFeed>>,
}

#[derive(serde::Deserialize)]
struct HermesParsedFeed {
    id: String,
    ema_price: HermesParsedPrice,
}

#[derive(serde::Deserialize)]
struct HermesParsedPrice {
    price: String,
    conf: String,
    expo: i32,
    publish_time: i64,
}

// ── RedStone gateway types ───────────────────────────────────────────────────

/// Default RedStone gateway URL.
const DEFAULT_REDSTONE_GATEWAY_URL: &str = "https://oracle-gateway-1.a.redstone.vip";

/// Default RedStone data service ID.
const REDSTONE_DATA_SERVICE_ID: &str = "redstone-primary-prod";

/// A single data point inside a RedStone gateway data package.
#[derive(serde::Deserialize)]
struct RedStoneGatewayDataPoint {
    value: f64,
}

/// A signed data package from the RedStone gateway.
#[derive(serde::Deserialize)]
struct RedStoneGatewayPackage {
    #[serde(rename = "dataPoints")]
    data_points: Vec<RedStoneGatewayDataPoint>,
    #[serde(rename = "timestampMilliseconds")]
    timestamp_milliseconds: u64,
}

// ── Shared types ─────────────────────────────────────────────────────────────

/// Shared cache of detected proxy oracle accounts.
pub type ProxyOracleCache =
    std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<AccountId>>>;

/// Oracle price fetcher.
///
/// Fetches prices directly from HTTP APIs (Pyth Hermes, RedStone gateway).
/// Supports LST oracles with transformers and proxy oracles with off-chain
/// aggregation.
pub struct OracleFetcher {
    client: JsonRpcClient,
    /// Cache of which oracles are LST oracles (`oracle_account` -> `underlying_oracle`)
    lst_oracle_cache: std::sync::Arc<tokio::sync::RwLock<HashMap<AccountId, Option<AccountId>>>>,
    /// Cache of detected proxy oracles (oracles that use cross-contract calls).
    /// Shared across all `OracleFetcher` instances so detection during registry
    /// refresh propagates to per-market fetchers.
    proxy_oracle_cache: ProxyOracleCache,
    /// HTTP client for API calls
    http_client: reqwest::Client,
    /// Pyth Hermes API URL (e.g., <https://hermes.pyth.network>)
    hermes_url: String,
    /// RedStone gateway URL for fetching fresh prices directly
    redstone_gateway_url: String,
}

impl OracleFetcher {
    /// Creates a new oracle fetcher.
    ///
    /// `proxy_oracle_cache` allows sharing the proxy oracle cache across multiple
    /// `OracleFetcher` instances. Pass `None` to create a standalone cache.
    pub fn new(
        client: JsonRpcClient,
        hermes_url: Option<String>,
        redstone_gateway_url: Option<String>,
        proxy_oracle_cache: Option<ProxyOracleCache>,
    ) -> Self {
        Self {
            client,
            lst_oracle_cache: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            proxy_oracle_cache: proxy_oracle_cache.unwrap_or_else(|| {
                std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()))
            }),
            http_client: reqwest::Client::new(),
            hermes_url: hermes_url.unwrap_or_else(|| "https://hermes.pyth.network".to_string()),
            redstone_gateway_url: redstone_gateway_url
                .unwrap_or_else(|| DEFAULT_REDSTONE_GATEWAY_URL.to_string()),
        }
    }

    /// Returns a clone of the shared proxy oracle cache handle.
    pub fn proxy_oracle_cache(&self) -> ProxyOracleCache {
        self.proxy_oracle_cache.clone()
    }

    /// Detects whether an oracle is a proxy oracle by checking if its account
    /// name starts with `proxy-oracle-`. Proxy oracles are deployed via the
    /// registry with this naming convention.
    pub async fn detect_and_register_proxy_oracle(&self, oracle: &AccountId) {
        if oracle.as_str().starts_with("proxy-oracle-")
            && self.proxy_oracle_cache.write().await.insert(oracle.clone())
        {
            tracing::info!(
                oracle = %oracle,
                "Registered proxy oracle"
            );
        }
    }

    /// Checks if the oracle is an LST oracle by attempting to fetch its underlying oracle ID.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn is_lst_oracle(&self, oracle: &AccountId) -> LiquidatorResult<Option<AccountId>> {
        // Check cache first
        {
            let cache = self.lst_oracle_cache.read().await;
            if let Some(cached) = cache.get(oracle) {
                return Ok(cached.clone());
            }
        }

        // Try to fetch underlying oracle ID
        let underlying_oracle: Result<AccountId, _> =
            view(&self.client, oracle.clone(), "oracle_id", json!({})).await;

        let result = if let Ok(underlying) = underlying_oracle {
            tracing::debug!(
                oracle = %oracle,
                underlying = %underlying,
                "Detected LST oracle"
            );
            Some(underlying)
        } else {
            tracing::debug!(oracle = %oracle, "Standard Pyth oracle (no oracle_id method)");
            None
        };

        // Cache the result
        {
            let mut cache = self.lst_oracle_cache.write().await;
            cache.insert(oracle.clone(), result.clone());
        }

        Ok(result)
    }

    // ── Pyth / Hermes ────────────────────────────────────────────────────────

    /// Fetches EMA prices from the Pyth Hermes HTTP API.
    ///
    /// Returns an `OracleResponse` keyed by `PriceIdentifier`.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn fetch_pyth_prices_from_hermes(
        &self,
        price_ids: &[PriceIdentifier],
    ) -> Option<OracleResponse> {
        let url = format!("{}/v2/updates/price/latest", self.hermes_url);
        let mut query_params: Vec<(&str, String)> = price_ids
            .iter()
            .map(|id| ("ids[]", id.to_string()))
            .collect();
        query_params.push(("parsed", "true".to_string()));

        let response = self
            .http_client
            .get(&url)
            .query(&query_params)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| {
                tracing::debug!(error = %e, "Hermes HTTP request failed");
            })
            .ok()?;

        if !response.status().is_success() {
            tracing::debug!(status = %response.status(), "Hermes returned error status");
            return None;
        }

        let body: HermesResponse = response
            .json()
            .await
            .map_err(|e| {
                tracing::debug!(error = %e, "Failed to parse Hermes response");
            })
            .ok()?;

        let parsed = body.parsed?;
        let mut result = OracleResponse::new();

        for feed in &parsed {
            // Parse the hex ID back to a PriceIdentifier
            let id_bytes = hex::decode(&feed.id)
                .map_err(|e| {
                    tracing::warn!(id = %feed.id, error = %e, "Invalid hex price ID from Hermes");
                })
                .ok()?;
            if id_bytes.len() != 32 {
                continue;
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&id_bytes);
            let price_id = PriceIdentifier(arr);

            let price_val: i64 = feed.ema_price.price.parse().ok()?;
            let conf_val: u64 = feed.ema_price.conf.parse().ok()?;

            result.insert(
                price_id,
                Some(pyth::Price {
                    price: near_sdk::json_types::I64(price_val),
                    conf: near_sdk::json_types::U64(conf_val),
                    expo: feed.ema_price.expo,
                    publish_time: pyth::PythTimestamp::from_secs(feed.ema_price.publish_time),
                }),
            );
        }

        tracing::debug!(
            price_count = result.len(),
            "Fetched Pyth EMA prices from Hermes"
        );

        Some(result)
    }

    // ── Main entry point ─────────────────────────────────────────────────────

    /// Fetches current oracle prices.
    ///
    /// Detects oracle type and uses the appropriate method:
    /// - LST oracles: Fetch from underlying oracle and apply transformers
    /// - Proxy oracles: Fetch from underlying oracles via proxy configuration
    /// - Pyth oracles: Hermes HTTP API
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        // Check if this is an LST oracle upfront
        if let Some(underlying_oracle) = self.is_lst_oracle(&oracle).await? {
            tracing::debug!(
                oracle = %oracle,
                underlying = %underlying_oracle,
                "Using LST oracle approach with transformers"
            );
            return self
                .get_oracle_prices_with_transformers(oracle, price_ids, age, underlying_oracle)
                .await;
        }

        // Check if this is a cached proxy oracle
        if self.proxy_oracle_cache.read().await.contains(&oracle) {
            return self.get_proxy_oracle_prices(oracle, price_ids, age).await;
        }

        // Standard Pyth oracle — fetch from Hermes HTTP API
        self.fetch_pyth_prices_from_hermes(price_ids)
            .await
            .ok_or_else(|| {
                LiquidatorError::PriceFetchError(crate::rpc::RpcError::WrongResponseKind(format!(
                    "Failed to fetch Pyth prices from Hermes for oracle {oracle}"
                )))
            })
    }

    // ── LST oracle ───────────────────────────────────────────────────────────

    /// Fetches prices from LST oracle by calling underlying Pyth oracle and applying transformers.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_oracle_prices_with_transformers(
        &self,
        lst_oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
        underlying_oracle: AccountId,
    ) -> LiquidatorResult<OracleResponse> {
        tracing::info!(
            oracle = %lst_oracle,
            underlying = %underlying_oracle,
            "Fetching LST oracle prices with transformers"
        );

        // Get transformers for each price ID
        let mut transformers: HashMap<PriceIdentifier, PriceTransformer> = HashMap::new();
        let mut underlying_price_ids: Vec<PriceIdentifier> = Vec::new();

        for &price_id in price_ids {
            match view::<Option<PriceTransformer>>(
                &self.client,
                lst_oracle.clone(),
                "get_transformer",
                json!({ "price_identifier": price_id }),
            )
            .await
            {
                Ok(Some(transformer)) => {
                    tracing::debug!(
                        price_id = ?price_id,
                        underlying_id = ?transformer.price_id,
                        "Found price transformer"
                    );
                    underlying_price_ids.push(transformer.price_id);
                    transformers.insert(price_id, transformer);
                }
                Ok(None) => {
                    tracing::debug!(price_id = ?price_id, "No transformer, using price ID as-is");
                    underlying_price_ids.push(price_id);
                }
                Err(e) => {
                    tracing::warn!(
                        price_id = ?price_id,
                        error = %e,
                        "Failed to get transformer, skipping market"
                    );
                    return Ok(HashMap::new());
                }
            }
        }

        tracing::debug!(
            underlying_oracle = %underlying_oracle,
            underlying_price_ids = ?underlying_price_ids,
            "Fetching prices from underlying Pyth oracle"
        );

        // Fetch prices from underlying Pyth oracle
        let mut underlying_prices =
            Box::pin(self.get_oracle_prices(underlying_oracle.clone(), &underlying_price_ids, age))
                .await?;

        if underlying_prices.is_empty() {
            tracing::warn!("Underlying oracle returned no prices, skipping market");
            return Ok(HashMap::new());
        }

        // Apply transformers to get final prices
        let mut final_prices: OracleResponse = HashMap::new();

        for (&original_price_id, transformer) in &transformers {
            if let Some(Some(underlying_price)) = underlying_prices.remove(&transformer.price_id) {
                // Fetch the input value for transformation
                match self
                    .fetch_transformer_input(&transformer.call, &lst_oracle)
                    .await
                {
                    Ok(input) => {
                        if let Some(transformed_price) =
                            transformer.action.apply(underlying_price, input)
                        {
                            tracing::debug!(
                                price_id = ?original_price_id,
                                "Successfully transformed price"
                            );
                            final_prices.insert(original_price_id, Some(transformed_price));
                        } else {
                            tracing::warn!(
                                price_id = ?original_price_id,
                                "Price transformation returned None"
                            );
                            final_prices.insert(original_price_id, None);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            price_id = ?original_price_id,
                            error = %e,
                            "Failed to fetch transformer input"
                        );
                        final_prices.insert(original_price_id, None);
                    }
                }
            } else {
                tracing::warn!(
                    price_id = ?original_price_id,
                    underlying_id = ?transformer.price_id,
                    "Underlying price not found in oracle response"
                );
                final_prices.insert(original_price_id, None);
            }
        }

        // Add prices that didn't need transformation
        for &price_id in price_ids {
            if !transformers.contains_key(&price_id) {
                if let Some(price) = underlying_prices.remove(&price_id) {
                    final_prices.insert(price_id, price);
                }
            }
        }

        tracing::info!(
            oracle = %lst_oracle,
            price_count = final_prices.len(),
            "Successfully fetched and transformed LST oracle prices"
        );

        Ok(final_prices)
    }

    // ── Proxy oracle ─────────────────────────────────────────────────────────

    /// Fetches prices from a proxy oracle by reading its configuration and querying
    /// underlying oracles (Pyth/RedStone) directly, then applying aggregation off-chain.
    ///
    /// Proxy oracles aggregate prices from multiple sources (Pyth + RedStone) using
    /// cross-contract calls on-chain, which fails in view mode. This method replicates
    /// the aggregation off-chain by:
    /// 1. Reading proxy config (`get_proxy`) for each price ID
    /// 2. Fetching prices from underlying oracles directly
    /// 3. Applying transformers (e.g., LST redemption rates)
    /// 4. Running the aggregation algorithm locally
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_proxy_oracle_prices(
        &self,
        proxy_oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        let mut result = OracleResponse::new();

        for &price_id in price_ids {
            let proxy: Option<Proxy> = view(
                &self.client,
                proxy_oracle.clone(),
                "get_proxy",
                json!({ "id": price_id }),
            )
            .await
            .map_err(LiquidatorError::PriceFetchError)?;

            let Some(proxy) = proxy else {
                tracing::warn!(
                    oracle = %proxy_oracle,
                    price_id = ?price_id,
                    "No proxy configuration found for price ID"
                );
                result.insert(price_id, None);
                continue;
            };

            // Collect prices from underlying oracles for each entry
            let mut prices: Vec<(pyth::Price, u32)> = Vec::new();

            for entry in &proxy.entries {
                let price = match &entry.source {
                    Source::Request(request) => self.fetch_oracle_request_price(request, age).await,
                    Source::Transformer(transformer) => {
                        self.fetch_proxy_transformed_price(transformer, age).await
                    }
                };

                if let Some(price) = price {
                    prices.push((price, entry.weight));
                }
            }

            // Apply aggregation using the same logic as the on-chain proxy
            let now = system_nanoseconds();
            let aggregated = proxy.aggregator.aggregate(&prices, now);
            result.insert(price_id, aggregated.map(Into::into));

            if result.get(&price_id).and_then(|p| p.as_ref()).is_some() {
                tracing::debug!(
                    oracle = %proxy_oracle,
                    price_id = ?price_id,
                    source_count = prices.len(),
                    "Proxy oracle: aggregated price from underlying sources"
                );
            } else {
                tracing::warn!(
                    oracle = %proxy_oracle,
                    price_id = ?price_id,
                    source_count = prices.len(),
                    "Proxy oracle: aggregation returned no price"
                );
            }
        }

        Ok(result)
    }

    // ── Individual source fetchers ───────────────────────────────────────────

    /// Fetches a price from a single oracle request (Pyth or RedStone).
    ///
    /// For Pyth requests, calls `get_oracle_prices` directly on the underlying
    /// Pyth oracle (not the proxy), which avoids infinite recursion since a
    /// real Pyth oracle won't trigger the proxy path.
    async fn fetch_oracle_request_price(
        &self,
        request: &OracleRequest,
        age: u32,
    ) -> Option<pyth::Price> {
        match request {
            OracleRequest::Pyth(pyth_req) => {
                // Use Box::pin to break the recursive async type cycle:
                // get_oracle_prices → get_proxy_oracle_prices → fetch_oracle_request_price → get_oracle_prices
                let response = Box::pin(self.get_oracle_prices(
                    pyth_req.oracle_id.clone(),
                    &[pyth_req.price_id],
                    age,
                ))
                .await
                .ok()?;
                response.get(&pyth_req.price_id)?.clone()
            }
            OracleRequest::RedStone(rs_req) => {
                self.fetch_redstone_price_from_gateway(&rs_req.price_id)
                    .await
            }
        }
    }

    /// Fetches a fresh price directly from the RedStone gateway HTTP API.
    ///
    /// The gateway returns signed data packages from multiple signers.
    /// We take the median price across packages for robustness, and use
    /// the package timestamp to construct a fresh `pyth::Price`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap
    )]
    async fn fetch_redstone_price_from_gateway(
        &self,
        feed_id: &redstone::FeedId,
    ) -> Option<pyth::Price> {
        let url = format!(
            "{}/v2/data-packages/latest/{}",
            self.redstone_gateway_url, REDSTONE_DATA_SERVICE_ID,
        );

        let response = self
            .http_client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    feed_id = %feed_id,
                    error = %e,
                    "RedStone gateway HTTP request failed"
                );
            })
            .ok()?;

        if !response.status().is_success() {
            tracing::warn!(
                feed_id = %feed_id,
                status = %response.status(),
                "RedStone gateway returned error status"
            );
            return None;
        }

        let body: HashMap<String, Vec<RedStoneGatewayPackage>> = response
            .json()
            .await
            .map_err(|e| {
                tracing::warn!(
                    feed_id = %feed_id,
                    error = %e,
                    "Failed to parse RedStone gateway response"
                );
            })
            .ok()?;

        let feed_id_str: &str = feed_id;
        let packages = body.get(feed_id_str)?;

        if packages.is_empty() {
            tracing::warn!(feed_id = %feed_id, "No data packages from RedStone gateway");
            return None;
        }

        // Extract prices and timestamp from all packages
        let mut values: Vec<f64> = packages
            .iter()
            .filter_map(|pkg| pkg.data_points.first().map(|dp| dp.value))
            .collect();

        if values.is_empty() {
            return None;
        }

        // Use median price for robustness
        values.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = values[values.len() / 2];

        // Use the timestamp from the first package (all packages share the same timestamp)
        let timestamp_ms = packages[0].timestamp_milliseconds;

        // Convert price to i64 mantissa with 8-decimal exponent.
        // RedStone prices use 8 decimals, so multiply by 10^8.
        let raw_value = (median * 1e8) as i64;

        let price = pyth::Price {
            price: near_sdk::json_types::I64(raw_value),
            conf: near_sdk::json_types::U64(0),
            expo: -8,
            publish_time: pyth::PythTimestamp::from_ms(timestamp_ms as i64),
        };

        tracing::debug!(
            feed_id = %feed_id,
            price = raw_value,
            timestamp_ms = timestamp_ms,
            signer_count = packages.len(),
            "Fetched fresh RedStone price from gateway"
        );

        Some(price)
    }

    // ── Transformers ─────────────────────────────────────────────────────────

    /// Fetches a transformed price from a proxy entry (underlying oracle + transformer input).
    async fn fetch_proxy_transformed_price(
        &self,
        transformer: &templar_common::oracle::price_transformer::ProxyPriceTransformer,
        age: u32,
    ) -> Option<pyth::Price> {
        // Fetch the underlying price
        let underlying = self
            .fetch_oracle_request_price(&transformer.request, age)
            .await?;

        // Fetch the transformer input (e.g., LST redemption rate).
        // The dummy account is needed for the trait method signature but unused in practice.
        #[allow(deprecated)]
        let dummy_account = AccountId::new_unvalidated(String::new());
        let input = self
            .fetch_transformer_input(&transformer.call, &dummy_account)
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = ?e,
                    "Failed to fetch proxy transformer input"
                );
            })
            .ok()?;

        transformer.action.apply(underlying, input)
    }

    /// Fetches the input value needed for price transformation (e.g., LST redemption rate).
    async fn fetch_transformer_input(
        &self,
        call: &templar_common::oracle::price_transformer::Call,
        _oracle: &AccountId,
    ) -> Result<Decimal, RpcError> {
        // Use the rpc_call() method to create a view query
        let query = call.rpc_call();

        // Execute the query using the RPC client
        let request = near_jsonrpc_client::methods::query::RpcQueryRequest {
            block_reference: near_primitives::types::BlockReference::latest(),
            request: query,
        };

        let response = self.client.call(request).await.map_err(RpcError::from)?;

        // Parse the result
        if let near_jsonrpc_primitives::types::query::QueryResponseKind::CallResult(result) =
            response.kind
        {
            let value: Decimal = near_sdk::serde_json::from_slice(&result.result)
                .map_err(RpcError::DeserializeError)?;
            Ok(value)
        } else {
            Err(RpcError::WrongResponseKind(
                "Expected CallResult".to_string(),
            ))
        }
    }
}

/// Returns the current system time as `Nanoseconds` (off-chain equivalent of `Nanoseconds::now()`).
#[allow(clippy::cast_possible_truncation)]
fn system_nanoseconds() -> Nanoseconds {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Nanoseconds::from_ns(dur.as_nanos() as u64)
}
