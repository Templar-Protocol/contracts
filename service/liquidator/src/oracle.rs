//! Oracle price fetching module.
//!
//! Handles fetching prices from various oracle types including:
//! - Standard Pyth oracles
//! - LST oracles with price transformers

use near_jsonrpc_client::JsonRpcClient;
use near_sdk::{serde_json::json, AccountId};
use std::collections::HashMap;
use templar_common::{
    number::Decimal,
    oracle::{
        price_transformer::PriceTransformer,
        pyth::{OracleResponse, PriceIdentifier},
    },
};

use crate::{
    rpc::{view, RpcError},
    LiquidatorError, LiquidatorResult,
};

/// Oracle price fetcher.
///
/// Responsible for:
/// - Fetching prices from Pyth oracles
/// - Handling LST oracles with transformers
/// - Applying price transformations
pub struct OracleFetcher {
    client: JsonRpcClient,
    /// Cache of which oracles are LST oracles (`oracle_account` -> `underlying_oracle`)
    lst_oracle_cache: std::sync::Arc<tokio::sync::RwLock<HashMap<AccountId, Option<AccountId>>>>,
}

impl OracleFetcher {
    /// Creates a new oracle fetcher.
    pub fn new(client: JsonRpcClient) -> Self {
        Self {
            client,
            lst_oracle_cache: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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

    /// Fetches current oracle prices.
    ///
    /// Detects oracle type and uses the appropriate method:
    /// - LST oracles: Fetch from underlying oracle and apply transformers
    /// - Pyth oracles: Direct fetch with `list_ema_prices_unsafe` or `list_ema_prices_no_older_than`
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

        // Standard Pyth oracle - try unsafe method first (faster)
        let result: Result<OracleResponse, _> = view(
            &self.client,
            oracle.clone(),
            "list_ema_prices_unsafe",
            json!({ "price_ids": price_ids }),
        )
        .await;

        match result {
            Ok(response) => Ok(response),
            Err(e) => {
                let error_msg = format!("{e:?}");
                tracing::debug!("First oracle call failed for {}: {}", oracle, error_msg);

                // If method not found, try the standard method with age validation
                if error_msg.contains("MethodNotFound") || error_msg.contains("MethodResolveError")
                {
                    tracing::debug!(
                        "Oracle {} doesn't support list_ema_prices_unsafe, trying list_ema_prices_no_older_than",
                        oracle
                    );

                    match view(
                        &self.client,
                        oracle.clone(),
                        "list_ema_prices_no_older_than",
                        json!({ "price_ids": price_ids, "age": age }),
                    )
                    .await
                    {
                        Ok(response) => {
                            tracing::info!(
                                "Successfully fetched prices from {} using list_ema_prices_no_older_than",
                                oracle
                            );
                            Ok(response)
                        }
                        Err(fallback_err) => Err(LiquidatorError::PriceFetchError(fallback_err)),
                    }
                } else {
                    Err(LiquidatorError::PriceFetchError(e))
                }
            }
        }
    }

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
