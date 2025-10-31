// SPDX-License-Identifier: MIT
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
use tracing::{debug, info, warn};

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
}

impl OracleFetcher {
    /// Creates a new oracle fetcher.
    pub fn new(client: JsonRpcClient) -> Self {
        Self { client }
    }

    /// Fetches current oracle prices.
    ///
    /// Tries multiple methods in order:
    /// 1. `list_ema_prices_unsafe` (Pyth oracle, potentially stale but fast)
    /// 2. `list_ema_prices_no_older_than` (Pyth oracle with age validation)
    /// 3. LST oracle approach with transformers
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn get_oracle_prices(
        &self,
        oracle: AccountId,
        price_ids: &[PriceIdentifier],
        age: u32,
    ) -> LiquidatorResult<OracleResponse> {
        // Try `list_ema_prices_unsafe` first (Pyth oracle)
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
                debug!("First oracle call failed for {}: {}", oracle, error_msg);

                // Check if oracle creates promises in view calls
                if error_msg.contains("ProhibitedInView") {
                    debug!(
                        oracle = %oracle,
                        "Oracle creates promises in view calls, trying LST oracle approach"
                    );
                    return self
                        .get_oracle_prices_with_transformers(oracle, price_ids, age)
                        .await;
                }

                // If method not found, try the standard method with age validation
                if error_msg.contains("MethodNotFound") || error_msg.contains("MethodResolveError")
                {
                    debug!(
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
                            info!(
                                "Successfully fetched prices from {} using list_ema_prices_no_older_than",
                                oracle
                            );
                            Ok(response)
                        }
                        Err(fallback_err) => {
                            let fallback_error_msg = format!("{fallback_err:?}");

                            // Check if fallback also fails with ProhibitedInView
                            if fallback_error_msg.contains("ProhibitedInView") {
                                debug!(
                                    oracle = %oracle,
                                    "Fallback also creates promises, trying LST oracle approach"
                                );
                                return self
                                    .get_oracle_prices_with_transformers(oracle, price_ids, age)
                                    .await;
                            }
                            Err(LiquidatorError::PriceFetchError(fallback_err))
                        }
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
    ) -> LiquidatorResult<OracleResponse> {
        info!(
            oracle = %lst_oracle,
            "Detected LST oracle, fetching transformers and applying manually"
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
                    debug!(
                        price_id = ?price_id,
                        underlying_id = ?transformer.price_id,
                        "Found price transformer"
                    );
                    underlying_price_ids.push(transformer.price_id);
                    transformers.insert(price_id, transformer);
                }
                Ok(None) => {
                    debug!(price_id = ?price_id, "No transformer, using price ID as-is");
                    underlying_price_ids.push(price_id);
                }
                Err(e) => {
                    warn!(
                        price_id = ?price_id,
                        error = %e,
                        "Failed to get transformer, skipping market"
                    );
                    return Ok(HashMap::new());
                }
            }
        }

        // Get underlying oracle account ID
        let underlying_oracle: AccountId =
            match view(&self.client, lst_oracle.clone(), "oracle_id", json!({})).await {
                Ok(oracle_id) => oracle_id,
                Err(e) => {
                    warn!(
                        oracle = %lst_oracle,
                        error = %e,
                        "Failed to get underlying oracle ID, skipping market"
                    );
                    return Ok(HashMap::new());
                }
            };

        debug!(
            underlying_oracle = %underlying_oracle,
            underlying_price_ids = ?underlying_price_ids,
            "Fetching prices from underlying Pyth oracle"
        );

        // Fetch prices from underlying Pyth oracle (use Box::pin to avoid infinite recursion)
        let mut underlying_prices =
            Box::pin(self.get_oracle_prices(underlying_oracle.clone(), &underlying_price_ids, age))
                .await?;

        if underlying_prices.is_empty() {
            warn!("Underlying oracle returned no prices, skipping market");
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
                            debug!(
                                price_id = ?original_price_id,
                                "Successfully transformed price"
                            );
                            final_prices.insert(original_price_id, Some(transformed_price));
                        } else {
                            warn!(
                                price_id = ?original_price_id,
                                "Price transformation returned None"
                            );
                            final_prices.insert(original_price_id, None);
                        }
                    }
                    Err(e) => {
                        warn!(
                            price_id = ?original_price_id,
                            error = %e,
                            "Failed to fetch transformer input"
                        );
                        final_prices.insert(original_price_id, None);
                    }
                }
            } else {
                warn!(
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

        info!(
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
