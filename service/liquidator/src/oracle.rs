//! Oracle price fetching module.
//!
//! Handles fetching prices from various oracle types including:
//! - Pyth oracles (via Hermes HTTP API)
//! - RedStone-backed feeds through proxy oracle cache reads
//! - LST oracles with price transformers
//! - Proxy oracles with cached on-chain aggregation

use near_sdk::AccountId;
use std::collections::{HashMap, HashSet};
use templar_common::{
    oracle::pyth::{self, OracleResponse, PriceIdentifier},
    Decimal,
};
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::{contract, lst_oracle, proxy_oracle, pyth as pyth_spec};
use templar_gateway_types::{
    common::ContractArgs, Base64Bytes, ContractMethodName, OperationStatus,
};
use templar_proxy_oracle_near_common::{
    input::Source,
    price_transformer::{Call, PriceTransformer},
    request::OracleRequest,
};

use crate::{
    rpc::{gateway_is_method_not_found, RpcError},
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

/// Binary (VAA) response from Pyth Hermes for on-chain price updates.
#[derive(serde::Deserialize)]
struct HermesBinaryResponse {
    binary: HermesBinaryData,
}

#[derive(serde::Deserialize)]
struct HermesBinaryData {
    data: Vec<String>,
}

// ── Shared types ─────────────────────────────────────────────────────────────

/// Shared cache of detected proxy oracle accounts.
pub type ProxyOracleCache =
    std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<AccountId>>>;

/// Oracle price fetcher.
///
/// Fetches prices directly from Pyth Hermes.
/// Supports LST oracles with transformers and proxy oracles with cached on-chain
/// aggregation.
pub struct OracleFetcher {
    client: SigningClient,
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
}

impl OracleFetcher {
    /// Creates a new oracle fetcher.
    ///
    /// `proxy_oracle_cache` allows sharing the proxy oracle cache across multiple
    /// `OracleFetcher` instances. Pass `None` to create a standalone cache.
    ///
    /// On-chain price pushes are signed by the account bound to the shared
    /// [`SigningClient`].
    pub fn new(
        client: SigningClient,
        hermes_url: Option<String>,
        _redstone_gateway_url: Option<String>,
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
        }
    }

    /// Returns a clone of the shared proxy oracle cache handle.
    pub fn proxy_oracle_cache(&self) -> ProxyOracleCache {
        self.proxy_oracle_cache.clone()
    }

    /// Detects whether an oracle is a proxy oracle by probing its view interface.
    pub async fn detect_and_register_proxy_oracle(&self, oracle: &AccountId) {
        if let Err(error) = self.is_proxy_oracle(oracle).await {
            tracing::warn!(%oracle, %error, "Failed to detect proxy oracle interface");
        }
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn is_proxy_oracle(&self, oracle: &AccountId) -> LiquidatorResult<bool> {
        if self.proxy_oracle_cache.read().await.contains(oracle) {
            return Ok(true);
        }

        match self
            .client
            .read(proxy_oracle::ListProxies {
                oracle_id: oracle.clone(),
                offset: None,
                count: Some(1),
            })
            .await
        {
            Ok(_) => {
                if self.proxy_oracle_cache.write().await.insert(oracle.clone()) {
                    tracing::info!(%oracle, "Registered proxy oracle");
                }
                Ok(true)
            }
            Err(error) if gateway_is_method_not_found(&error) => Ok(false),
            Err(error) => Err(LiquidatorError::PriceFetchError(error.into())),
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
        let result = if let Ok(response) = self
            .client
            .read(lst_oracle::GetOracleId {
                oracle_id: oracle.clone(),
            })
            .await
        {
            tracing::debug!(
                oracle = %oracle,
                underlying = %response.pyth_oracle_id,
                "Detected LST oracle"
            );
            Some(response.pyth_oracle_id)
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
            let Ok(id_bytes) = hex::decode(&feed.id).map_err(|e| {
                tracing::warn!(id = %feed.id, error = %e, "Invalid hex price ID from Hermes");
            }) else {
                continue;
            };
            if id_bytes.len() != 32 {
                continue;
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&id_bytes);
            let price_id = PriceIdentifier(arr);

            let (Ok(price_val), Ok(conf_val)) = (
                feed.ema_price.price.parse::<i64>(),
                feed.ema_price.conf.parse::<u64>(),
            ) else {
                tracing::warn!(id = %feed.id, "Invalid Hermes price payload, skipping feed");
                continue;
            };

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

    // ── On-chain price updates ────────────────────────────────────────────────

    /// Resolves the market-facing oracle account + price IDs to the actual
    /// underlying Pyth oracle and feed IDs that need `update_price_feeds`.
    ///
    /// - **Direct Pyth oracle**: returns as-is.
    /// - **LST oracle**: resolves via `oracle_id()` + transformers to get
    ///   the underlying Pyth oracle and transformed feed IDs.
    /// - **Proxy oracle**: reads proxy entries, collects all
    ///   `OracleRequest::Pyth` targets (`oracle_id` + `price_id`).
    ///
    /// Returns a map of `pyth_oracle_account` → `Vec<feed_ids>`.
    pub async fn resolve_pyth_update_targets(
        &self,
        oracle: &AccountId,
        price_ids: &[PriceIdentifier],
    ) -> HashMap<AccountId, Vec<PriceIdentifier>> {
        let mut targets: HashMap<AccountId, HashSet<PriceIdentifier>> = HashMap::new();

        // LST oracle: resolve underlying oracle + transform price IDs
        if let Ok(Some(underlying_oracle)) = self.is_lst_oracle(oracle).await {
            let mut underlying_ids = Vec::new();
            for &pid in price_ids {
                match self
                    .client
                    .read(lst_oracle::GetTransformer {
                        oracle_id: oracle.clone(),
                        price_identifier: pid,
                    })
                    .await
                {
                    Ok(result) => match result.transformer {
                        Some(transformer) => underlying_ids.push(transformer.price_id),
                        None => underlying_ids.push(pid),
                    },
                    Err(_) => underlying_ids.push(pid),
                }
            }
            targets
                .entry(underlying_oracle)
                .or_default()
                .extend(underlying_ids);
            return targets
                .into_iter()
                .map(|(oracle_id, feed_ids)| (oracle_id, feed_ids.into_iter().collect()))
                .collect();
        }

        // Proxy oracle: collect Pyth entries from proxy config
        if self.proxy_oracle_cache.read().await.contains(oracle) {
            for &pid in price_ids {
                match self
                    .client
                    .read(proxy_oracle::GetProxy {
                        oracle_id: oracle.clone(),
                        id: pid,
                    })
                    .await
                {
                    Ok(result) => {
                        if let Some(proxy) = result.proxy {
                            for source in proxy.sources() {
                                Self::collect_pyth_targets_from_source(source, &mut targets);
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(oracle = %oracle, price_id = ?pid, error = %error, "Failed to read proxy configuration while resolving Pyth targets");
                    }
                }
            }
            return targets
                .into_iter()
                .map(|(oracle_id, feed_ids)| (oracle_id, feed_ids.into_iter().collect()))
                .collect();
        }

        // Direct Pyth oracle
        targets
            .entry(oracle.clone())
            .or_default()
            .extend(price_ids.iter().copied());
        targets
            .into_iter()
            .map(|(oracle_id, feed_ids)| (oracle_id, feed_ids.into_iter().collect()))
            .collect()
    }

    /// Collects Pyth oracle targets from a proxy source entry.
    fn collect_pyth_targets_from_source(
        source: &Source,
        targets: &mut HashMap<AccountId, HashSet<PriceIdentifier>>,
    ) {
        match source {
            Source::Request(OracleRequest::Pyth(pyth_req)) => {
                targets
                    .entry(pyth_req.oracle_id.clone())
                    .or_default()
                    .insert(pyth_req.price_id);
            }
            Source::Request(OracleRequest::RedStone(_)) => {
                // RedStone prices are pushed by the relayer, not by us
            }
            Source::Transformer(transformer) => {
                // Transformer wraps an underlying request — extract its Pyth target
                Self::collect_pyth_targets_from_source(
                    &Source::Request(transformer.request.clone()),
                    targets,
                );
            }
        }
    }

    /// Resolves market-level oracle config to underlying Pyth targets and pushes
    /// fresh prices on-chain for each. Returns `Ok(true)` if any update was sent.
    pub async fn update_onchain_prices(
        &self,
        oracle: &AccountId,
        price_ids: &[PriceIdentifier],
    ) -> LiquidatorResult<bool> {
        let is_proxy_oracle = self.is_proxy_oracle(oracle).await?;
        let targets = self.resolve_pyth_update_targets(oracle, price_ids).await;

        if targets.is_empty() && !is_proxy_oracle {
            tracing::debug!("No Pyth targets to update on-chain");
            return Ok(false);
        }

        let mut any_updated = false;
        for (pyth_oracle, feed_ids) in &targets {
            match self.update_pyth_prices(pyth_oracle, feed_ids).await {
                Ok(true) => any_updated = true,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        oracle = %pyth_oracle,
                        error = %e,
                        "Failed to update on-chain Pyth prices; proceeding with existing on-chain state"
                    );
                }
            }
        }

        if is_proxy_oracle {
            any_updated |= self.update_proxy_prices(oracle, price_ids).await?;
        }

        Ok(any_updated)
    }

    /// Refreshes a proxy oracle cache by invoking its on-chain `update_prices` flow.
    ///
    /// Best-effort: the gateway write returns only the operation status, not the
    /// per-feed cache result, so an unsuccessful operation surfaces as an error
    /// that the caller logs and swallows.
    #[tracing::instrument(skip(self), level = "info")]
    async fn update_proxy_prices(
        &self,
        oracle: &AccountId,
        price_ids: &[PriceIdentifier],
    ) -> LiquidatorResult<bool> {
        let result = self
            .client
            .execute(proxy_oracle::UpdatePrices {
                oracle_id: oracle.clone(),
                price_ids: price_ids.to_vec(),
            })
            .await
            .map_err(|e| {
                LiquidatorError::OracleUpdateError(format!("Proxy oracle update failed: {e}"))
            })?;

        if result.operation.status != OperationStatus::Succeeded {
            return Err(LiquidatorError::OracleUpdateError(format!(
                "Proxy oracle update operation {} ended with status {:?}",
                result.operation.id.0, result.operation.status
            )));
        }

        tracing::info!(oracle = %oracle, price_ids = ?price_ids, operation_id = %result.operation.id.0, "Successfully updated proxy oracle prices");
        Ok(true)
    }

    /// Pushes fresh Pyth prices on-chain by fetching a VAA from Hermes and
    /// submitting an `update_price_feeds` transaction to the oracle contract.
    ///
    /// The market contract reads prices from the on-chain oracle during
    /// liquidation execution, so prices must be fresh there — not just in the
    /// liquidator's local HTTP-fetched view.
    ///
    /// Returns `Ok(true)` if the update was applied on-chain.
    #[tracing::instrument(skip(self), level = "info")]
    async fn update_pyth_prices(
        &self,
        oracle: &AccountId,
        price_ids: &[PriceIdentifier],
    ) -> LiquidatorResult<bool> {
        tracing::info!(
            oracle = %oracle,
            price_ids = ?price_ids,
            "Fetching VAA from Hermes for on-chain price update"
        );

        // Fetch binary VAA from Hermes
        let url = format!("{}/v2/updates/price/latest", self.hermes_url);
        let query_params: Vec<_> = price_ids
            .iter()
            .map(|id| ("ids[]", id.to_string()))
            .collect();

        let response = self
            .http_client
            .get(&url)
            .query(&query_params)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| LiquidatorError::OracleUpdateError(format!("Hermes API error: {e}")))?;

        if !response.status().is_success() {
            return Err(LiquidatorError::OracleUpdateError(format!(
                "Hermes API returned status: {}",
                response.status()
            )));
        }

        let body: HermesBinaryResponse = response.json().await.map_err(|e| {
            LiquidatorError::OracleUpdateError(format!("Failed to parse Hermes response: {e}"))
        })?;

        let vaa_hex = body.binary.data.first().ok_or_else(|| {
            LiquidatorError::OracleUpdateError("No VAA data in Hermes response".to_string())
        })?;

        // Hermes returns the update payload as a hex string. The gateway's
        // `pyth.updatePriceFeeds` carries the raw bytes (`Base64Bytes`) and
        // re-hex-encodes them for the on-chain `update_price_feeds` call, so the
        // hex string is decoded here to keep the on-chain bytes identical to the
        // pre-migration behaviour.
        let vaa_bytes = hex::decode(vaa_hex).map_err(|e| {
            LiquidatorError::OracleUpdateError(format!("Invalid VAA hex from Hermes: {e}"))
        })?;

        tracing::info!(
            vaa_size = vaa_bytes.len(),
            "Fetched VAA from Hermes, submitting to oracle"
        );

        match self
            .client
            .execute(pyth_spec::UpdatePriceFeeds {
                oracle_id: oracle.clone(),
                data: Base64Bytes(vaa_bytes),
            })
            .await
        {
            Ok(result) if result.operation.status == OperationStatus::Succeeded => {
                tracing::info!(
                    operation_id = %result.operation.id.0,
                    oracle = %oracle,
                    "Successfully updated on-chain Pyth prices"
                );
                Ok(true)
            }
            Ok(result) => {
                tracing::error!(
                    operation_id = %result.operation.id.0,
                    status = ?result.operation.status,
                    oracle = %oracle,
                    "Pyth price update did not succeed"
                );
                Err(LiquidatorError::OracleUpdateError(format!(
                    "Pyth price update operation {} ended with status {:?}",
                    result.operation.id.0, result.operation.status
                )))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    oracle = %oracle,
                    "Failed to submit price update transaction"
                );
                Err(LiquidatorError::OracleUpdateError(format!(
                    "Transaction failed: {e}"
                )))
            }
        }
    }

    // ── Main entry point ─────────────────────────────────────────────────────

    /// Fetches current oracle prices.
    ///
    /// Detects oracle type and uses the appropriate method:
    /// - LST oracles: Fetch from underlying oracle and apply transformers
    /// - Proxy oracles: Read cached on-chain proxy oracle prices
    /// - Pyth oracles: Hermes HTTP API
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        // Check proxy interface first so protected proxy feeds cannot be bypassed by cache misses
        // or nonstandard account naming.
        if self.is_proxy_oracle(&oracle).await? {
            return self.get_proxy_oracle_prices(oracle, price_ids, age).await;
        }

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
    #[allow(clippy::too_many_lines)]
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
            match self
                .client
                .read(lst_oracle::GetTransformer {
                    oracle_id: lst_oracle.clone(),
                    price_identifier: price_id,
                })
                .await
            {
                Ok(result) => {
                    if let Some(transformer) = result.transformer {
                        tracing::debug!(
                            price_id = ?price_id,
                            underlying_id = ?transformer.price_id,
                            "Found price transformer"
                        );
                        underlying_price_ids.push(transformer.price_id);
                        transformers.insert(price_id, transformer);
                    } else {
                        tracing::debug!(price_id = ?price_id, "No transformer, using price ID as-is");
                        underlying_price_ids.push(price_id);
                    }
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
                match self.fetch_transformer_input(&transformer.call).await {
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

    /// Fetches prices from a proxy oracle cache.
    ///
    /// Proxy oracle aggregation, circuit-breaker evaluation, and cache writes happen in
    /// the proxy contract's `update_prices` flow. This read path intentionally does not
    /// re-run proxy logic off-chain because that would bypass on-chain breaker state.
    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_proxy_oracle_prices(
        &self,
        proxy_oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        let result = self
            .client
            .read(pyth_spec::ListEmaPricesNoOlderThan {
                oracle_id: proxy_oracle,
                price_ids: price_ids.to_vec(),
                age: u64::from(age),
            })
            .await
            .map_err(|e| LiquidatorError::PriceFetchError(e.into()))?;

        Ok(result
            .prices
            .into_iter()
            .map(|entry| (entry.price_id, entry.price))
            .collect())
    }

    // ── Transformers ─────────────────────────────────────────────────────────

    /// Fetches the input value needed for price transformation (e.g., LST redemption rate).
    async fn fetch_transformer_input(&self, call: &Call) -> Result<Decimal, RpcError> {
        let contract_id = call.account_id.as_str().parse().map_err(|err| {
            RpcError::WrongResponseKind(format!("Invalid account ID in transformer call: {err}"))
        })?;

        let result = self
            .client
            .read(contract::ViewFunction {
                contract_id,
                method_name: ContractMethodName(call.method_name.clone()),
                args: ContractArgs::Raw(Base64Bytes(call.args.0.clone())),
            })
            .await
            .map_err(RpcError::from)?;

        let value: Decimal =
            near_sdk::serde_json::from_value(result.value).map_err(RpcError::DeserializeError)?;
        Ok(value)
    }
}
