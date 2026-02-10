//! Oracle price fetching module.
//!
//! Handles fetching prices from various oracle types including:
//! - Standard Pyth oracles
//! - LST oracles with price transformers
//! - Updating stale Pyth prices via Hermes API

use near_jsonrpc_client::{
    methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest, JsonRpcClient,
};
use near_primitives::{
    action::FunctionCallAction,
    transaction::{Transaction, TransactionV0},
    types::BlockReference,
};
use near_sdk::{serde_json::json, AccountId, NearToken};
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

#[derive(serde::Deserialize)]
struct HermesResponse {
    binary: HermesBinary,
}

#[derive(serde::Deserialize)]
struct HermesBinary {
    data: Vec<String>,
}

/// Oracle price fetcher.
///
/// Responsible for:
/// - Fetching prices from Pyth oracles
/// - Handling LST oracles with transformers
/// - Applying price transformations
/// - Updating stale Pyth prices via Hermes API
pub struct OracleFetcher {
    client: JsonRpcClient,
    /// Cache of which oracles are LST oracles (`oracle_account` -> `underlying_oracle`)
    lst_oracle_cache: std::sync::Arc<tokio::sync::RwLock<HashMap<AccountId, Option<AccountId>>>>,
    /// HTTP client for Hermes API
    http_client: reqwest::Client,
    /// Hermes API URL (e.g., <https://hermes.pyth.network>)
    hermes_url: String,
    /// Signer for updating oracle prices
    signer_id: Option<AccountId>,
    /// Private key for signing transactions
    signer_key: Option<near_crypto::SecretKey>,
}

impl OracleFetcher {
    /// Creates a new oracle fetcher.
    pub fn new(
        client: JsonRpcClient,
        hermes_url: Option<String>,
        signer_id: Option<AccountId>,
        signer_key: Option<near_crypto::SecretKey>,
    ) -> Self {
        Self {
            client,
            lst_oracle_cache: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            hermes_url: hermes_url.unwrap_or_else(|| "https://hermes.pyth.network".to_string()),
            signer_id,
            signer_key,
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

    /// Updates Pyth oracle prices by fetching latest data from Hermes and pushing to oracle contract.
    ///
    /// This method:
    /// 1. Fetches latest price updates (VAA) from Pyth Hermes API
    /// 2. Submits update transaction to the oracle contract
    ///
    /// Returns Ok(true) if update was sent, Ok(false) if no signer configured, Err on failure.
    #[tracing::instrument(skip(self), level = "info")]
    pub async fn update_pyth_prices(
        &self,
        oracle: &AccountId,
        price_ids: &[PriceIdentifier],
    ) -> LiquidatorResult<bool> {
        // Check if we have credentials to update
        let (Some(_signer_id), Some(_signer_key)) = (&self.signer_id, &self.signer_key) else {
            tracing::warn!("No signer configured, cannot update Pyth prices");
            return Ok(false);
        };

        tracing::info!(
            oracle = %oracle,
            price_ids = ?price_ids,
            hermes_url = %self.hermes_url,
            "Fetching latest price updates from Hermes"
        );

        // Build Hermes API request
        let url = format!("{}/v2/updates/price/latest", self.hermes_url);
        let query_params: Vec<_> = price_ids
            .iter()
            .map(|id| ("ids[]", id.to_string()))
            .collect();

        // Fetch VAA from Hermes
        let response = self
            .http_client
            .get(&url)
            .query(&query_params)
            .send()
            .await
            .map_err(|e| LiquidatorError::PriceUpdateError(format!("Hermes API error: {e}")))?;

        if !response.status().is_success() {
            return Err(LiquidatorError::PriceUpdateError(format!(
                "Hermes API returned status: {}",
                response.status()
            )));
        }

        let body: HermesResponse = response.json().await.map_err(|e| {
            LiquidatorError::PriceUpdateError(format!("Failed to parse Hermes response: {e}"))
        })?;

        let vaa_hex = body.binary.data.first().ok_or_else(|| {
            LiquidatorError::PriceUpdateError("No VAA data in Hermes response".to_string())
        })?;

        tracing::info!(
            vaa_size = vaa_hex.len(),
            "Successfully fetched VAA from Hermes, submitting to oracle"
        );

        // Submit update to oracle using NEAR transaction
        let signer_id = self.signer_id.as_ref().ok_or_else(|| {
            LiquidatorError::PriceUpdateError("No signer_id configured".to_string())
        })?;
        let signer_key = self.signer_key.as_ref().ok_or_else(|| {
            LiquidatorError::PriceUpdateError("No signer_key configured".to_string())
        })?;

        // Get current nonce
        let access_key_query_response = self
            .client
            .call(near_jsonrpc_client::methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: near_primitives::views::QueryRequest::ViewAccessKey {
                    account_id: signer_id.clone(),
                    public_key: signer_key.public_key(),
                },
            })
            .await
            .map_err(|e| {
                LiquidatorError::PriceUpdateError(format!("Failed to query access key: {e}"))
            })?;

        let current_nonce = match access_key_query_response.kind {
            near_jsonrpc_primitives::types::query::QueryResponseKind::AccessKey(access_key) => {
                access_key.nonce
            }
            _ => {
                return Err(LiquidatorError::PriceUpdateError(
                    "Unexpected query response kind".to_string(),
                ))
            }
        };

        // Get latest block hash
        let block = self
            .client
            .call(near_jsonrpc_client::methods::block::RpcBlockRequest {
                block_reference: BlockReference::latest(),
            })
            .await
            .map_err(|e| {
                LiquidatorError::PriceUpdateError(format!("Failed to get latest block: {e}"))
            })?;

        // Construct transaction
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: signer_id.clone(),
            public_key: signer_key.public_key(),
            nonce: current_nonce + 1,
            receiver_id: oracle.clone(),
            block_hash: block.header.hash,
            actions: vec![FunctionCallAction {
                method_name: "update_price_feeds".to_string(),
                args: near_sdk::serde_json::json!({
                    "data": vaa_hex
                })
                .to_string()
                .into_bytes(),
                gas: near_primitives::gas::Gas::from_teragas(100),
                deposit: NearToken::from_millinear(1),
            }
            .into()],
        });

        // Sign and send transaction
        let signer =
            near_crypto::InMemorySigner::from_secret_key(signer_id.clone(), signer_key.clone());
        let signed_transaction = transaction.sign(&signer);
        let request = RpcBroadcastTxCommitRequest { signed_transaction };

        match self.client.call(request).await {
            Ok(response) => {
                tracing::info!(
                    tx_hash = %response.transaction.hash,
                    oracle = %oracle,
                    "Successfully updated Pyth prices"
                );
                Ok(true)
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    oracle = %oracle,
                    "Failed to submit price update transaction"
                );
                Err(LiquidatorError::PriceUpdateError(format!(
                    "Transaction failed: {e}"
                )))
            }
        }
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
