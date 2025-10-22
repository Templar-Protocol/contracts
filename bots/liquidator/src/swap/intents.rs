// SPDX-License-Identifier: MIT
//! NEAR Intents swap provider implementation.
//!
//! NEAR Intents is a cross-chain intent-based transaction protocol that enables
//! users to specify desired outcomes (e.g., "swap X for Y") without managing
//! the underlying execution details. A network of solvers competes to fulfill
//! intents optimally.
//!
//! # Features
//!
//! - Cross-chain swaps without bridging
//! - Solver competition for best execution
//! - Support for 120+ assets across 20+ chains
//! - Atomic execution guarantees
//!
//! # Architecture
//!
//! The implementation uses Defuse Protocol's solver relay infrastructure:
//! 1. Request a quote from the solver network
//! 2. Solvers compete to provide best execution
//! 3. User signs the selected intent
//! 4. Solver executes and settles the swap atomically
//!
//! # References
//!
//! - Solver Relay API: <https://solver-relay-v2.chaindefuser.com/rpc>
//! - Documentation: <https://docs.near-intents.org>

use std::sync::Arc;
use std::time::Duration;

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    views::FinalExecutionStatus,
};
use near_sdk::{json_types::U128, serde_json, AccountId};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use templar_common::asset::{AssetClass, FungibleAsset};
use tracing::{debug, error, info, instrument};

use crate::rpc::{get_access_key_data, send_tx, AppError, AppResult, Network};

use super::SwapProvider;

/// JSON-RPC request structure for solver relay quote requests.
#[derive(Debug, Clone, Serialize)]
struct SolverQuoteRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: QuoteParams,
}

/// Parameters for quote request.
#[derive(Debug, Clone, Serialize)]
struct QuoteParams {
    /// Input asset identifier in Defuse format (e.g., "near:usdc.near")
    defuse_asset_identifier_in: String,
    /// Output asset identifier in Defuse format
    defuse_asset_identifier_out: String,
    /// Exact output amount desired (as string)
    exact_amount_out: String,
    /// Minimum deadline for quote validity in milliseconds
    min_deadline_ms: u64,
}

/// JSON-RPC response from solver relay.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct SolverQuoteResponse {
    jsonrpc: String,
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<QuoteResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// Successful quote result.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct QuoteResult {
    /// Input amount required (as string)
    input_amount: String,
    /// Output amount that will be received (as string)
    output_amount: String,
    /// Exchange rate
    #[serde(skip_serializing_if = "Option::is_none")]
    exchange_rate: Option<String>,
    /// Solver that provided the quote
    #[serde(skip_serializing_if = "Option::is_none")]
    solver_id: Option<String>,
    /// Quote expiration timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_ms: Option<u64>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// Intent message structure for NEAR Intents contract.
#[derive(Debug, Clone, Serialize)]
struct IntentMessage {
    /// Unique intent identifier
    intent_id: String,
    /// The action to perform
    action: IntentAction,
    /// Deadline timestamp in milliseconds
    deadline_ms: u128,
    /// Optional whitelist of solvers allowed to fulfill this intent
    #[serde(skip_serializing_if = "Option::is_none")]
    solver_whitelist: Option<Vec<String>>,
}

/// Intent action types.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum IntentAction {
    /// Swap between two assets
    Swap {
        from_asset: AssetSpec,
        to_asset: AssetSpec,
    },
}

/// Asset specification for intents.
#[derive(Debug, Clone, Serialize)]
struct AssetSpec {
    /// Defuse asset identifier (e.g., "near:usdc.near")
    defuse_asset_id: String,
    /// Amount (for input assets)
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<String>,
    /// Minimum amount (for output assets)
    #[serde(skip_serializing_if = "Option::is_none")]
    min_amount: Option<String>,
}

/// NEAR Intents swap provider using Defuse Protocol's solver network.
///
/// This provider enables cross-chain swaps through the NEAR Intents protocol,
/// leveraging a decentralized solver network for optimal execution.
///
/// # Configuration
///
/// The provider can be configured with custom solver relay endpoints and
/// timeout settings to match operational requirements.
#[derive(Debug, Clone)]
pub struct IntentsSwap {
    /// Defuse Protocol solver relay endpoint
    pub solver_relay_url: String,
    /// NEAR Intents contract account ID
    pub intents_contract: AccountId,
    /// JSON-RPC client for NEAR blockchain interaction
    pub client: JsonRpcClient,
    /// Transaction signer
    pub signer: Arc<Signer>,
    /// Quote request timeout in milliseconds
    pub quote_timeout_ms: u64,
    /// Maximum acceptable slippage in basis points (100 = 1%)
    pub max_slippage_bps: u32,
    /// HTTP client for solver relay communication
    pub http_client: Client,
}

impl IntentsSwap {
    /// Creates a new NEAR Intents swap provider with default settings.
    ///
    /// # Arguments
    ///
    /// * `client` - JSON-RPC client for blockchain communication
    /// * `signer` - Transaction signer
    /// * `network` - Target network (mainnet/testnet)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use templar_bots::swap::intents::IntentsSwap;
    /// # use near_jsonrpc_client::JsonRpcClient;
    /// # use templar_bots::Network;
    /// # use std::sync::Arc;
    /// let swap = IntentsSwap::new(
    ///     JsonRpcClient::connect("https://rpc.testnet.near.org"),
    ///     signer,
    ///     Network::Testnet,
    /// );
    /// ```
    pub fn new(client: JsonRpcClient, signer: Arc<Signer>, network: Network) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_millis(Self::DEFAULT_QUOTE_TIMEOUT_MS))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            solver_relay_url: Self::DEFAULT_SOLVER_RELAY_URL.to_string(),
            intents_contract: Self::intents_contract_for_network(network),
            client,
            signer,
            quote_timeout_ms: Self::DEFAULT_QUOTE_TIMEOUT_MS,
            max_slippage_bps: Self::DEFAULT_MAX_SLIPPAGE_BPS,
            http_client,
        }
    }

    /// Creates a new NEAR Intents swap provider with custom configuration.
    ///
    /// # Arguments
    ///
    /// * `solver_relay_url` - Custom solver relay endpoint
    /// * `intents_contract` - NEAR Intents contract account ID
    /// * `client` - JSON-RPC client
    /// * `signer` - Transaction signer
    /// * `quote_timeout_ms` - Quote request timeout in milliseconds
    #[allow(clippy::too_many_arguments)]
    pub fn with_config(
        solver_relay_url: String,
        intents_contract: AccountId,
        client: JsonRpcClient,
        signer: Arc<Signer>,
        quote_timeout_ms: u64,
        max_slippage_bps: u32,
    ) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_millis(quote_timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            solver_relay_url,
            intents_contract,
            client,
            signer,
            quote_timeout_ms,
            max_slippage_bps,
            http_client,
        }
    }

    /// Default solver relay endpoint (Defuse Protocol V2)
    pub const DEFAULT_SOLVER_RELAY_URL: &'static str =
        "https://solver-relay-v2.chaindefuser.com/rpc";

    /// Default quote timeout (60 seconds)
    pub const DEFAULT_QUOTE_TIMEOUT_MS: u64 = 60_000;

    /// Default maximum slippage (1% = 100 basis points)
    pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 100;

    /// Default transaction timeout in seconds
    const DEFAULT_TIMEOUT: u64 = 60;

    /// Mainnet NEAR Intents contract
    const MAINNET_INTENTS_CONTRACT: &'static str = "intents.near";

    /// Testnet NEAR Intents contract
    const TESTNET_INTENTS_CONTRACT: &'static str = "intents.testnet";

    /// Returns the appropriate intents contract for the network.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "Hardcoded contract IDs are always valid"
    )]
    fn intents_contract_for_network(network: Network) -> AccountId {
        match network {
            Network::Mainnet => Self::MAINNET_INTENTS_CONTRACT
                .parse()
                .expect("Mainnet intents contract ID is valid"),
            Network::Testnet => Self::TESTNET_INTENTS_CONTRACT
                .parse()
                .expect("Testnet intents contract ID is valid"),
        }
    }

    /// Converts a `FungibleAsset` to Defuse asset identifier format.
    ///
    /// Defuse asset identifiers follow the format:
    /// - NEAR NEP-141: `near:<contract_id>`
    /// - NEAR NEP-245: `near:<contract_id>/<token_id>`
    fn to_defuse_asset_id<A: AssetClass>(asset: &FungibleAsset<A>) -> String {
        match asset.clone().into_nep141() {
            Some(_) => format!("near:{}", asset.contract_id()),
            None => {
                // NEP-245
                if let Some((contract, token_id)) = asset.clone().into_nep245() {
                    format!("near:{contract}/{token_id}")
                } else {
                    // Fallback - should not happen with valid FungibleAsset
                    format!("near:{}", asset.contract_id())
                }
            }
        }
    }

    /// Requests a quote from the solver network via HTTP/JSON-RPC.
    ///
    /// This makes an actual HTTP call to the Defuse Protocol solver relay
    /// to get competitive quotes from the solver network.
    ///
    /// # Returns
    ///
    /// The input amount required to obtain the desired output amount.
    #[instrument(skip(self), level = "debug")]
    async fn request_quote_from_solver<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        let from_defuse_id = Self::to_defuse_asset_id(from_asset);
        let to_defuse_id = Self::to_defuse_asset_id(to_asset);

        // Build JSON-RPC request for solver relay
        let request = SolverQuoteRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "get_quote".to_string(),
            params: QuoteParams {
                defuse_asset_identifier_in: from_defuse_id.clone(),
                defuse_asset_identifier_out: to_defuse_id.clone(),
                exact_amount_out: output_amount.0.to_string(),
                min_deadline_ms: self.quote_timeout_ms,
            },
        };

        info!(
            from = %from_defuse_id,
            to = %to_defuse_id,
            output = %output_amount.0,
            relay_url = %self.solver_relay_url,
            "Requesting quote from NEAR Intents solver network"
        );

        // Make HTTP POST request to solver relay
        let response = self
            .http_client
            .post(&self.solver_relay_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                error!(?e, "Failed to send request to solver relay");
                AppError::ValidationError(format!("Solver relay request failed: {e}"))
            })?;

        // Check HTTP status
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(
                status = %status,
                body = %body,
                "Solver relay returned error status"
            );
            return Err(AppError::ValidationError(format!(
                "Solver relay HTTP error {status}: {body}"
            )));
        }

        // Parse JSON-RPC response
        let solver_response: SolverQuoteResponse = response.json().await.map_err(|e| {
            error!(?e, "Failed to parse solver relay response");
            AppError::ValidationError(format!("Invalid solver relay response: {e}"))
        })?;

        // Check for JSON-RPC error
        if let Some(error) = solver_response.error {
            error!(
                code = error.code,
                message = %error.message,
                "Solver relay returned JSON-RPC error"
            );
            return Err(AppError::ValidationError(format!(
                "Solver relay error {}: {}",
                error.code, error.message
            )));
        }

        // Extract result
        let result = solver_response.result.ok_or_else(|| {
            error!("Solver relay response missing result field");
            AppError::ValidationError("Solver relay response missing result".to_string())
        })?;

        // Parse input amount from string
        let input_amount: u128 = result.input_amount.parse().map_err(|e| {
            error!(?e, amount = %result.input_amount, "Failed to parse input amount");
            AppError::ValidationError(format!("Invalid input amount format: {e}"))
        })?;

        info!(
            input_amount = %input_amount,
            output_amount = %output_amount.0,
            exchange_rate = %(input_amount as f64 / output_amount.0 as f64),
            solver = %result.solver_id.unwrap_or_else(|| "unknown".to_string()),
            "Quote received from solver network"
        );

        Ok(U128(input_amount))
    }

    /// Creates an intent message for the NEAR Intents contract.
    ///
    /// The intent specifies the desired swap outcome, which solvers will compete
    /// to fulfill. This follows the NEAR Intents contract message format.
    fn create_intent_message<F: AssetClass, T: AssetClass>(
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        input_amount: U128,
        min_output_amount: U128,
    ) -> AppResult<String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Generate unique intent ID based on timestamp and assets
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let intent_id = format!(
            "intent_{}_{}_{}",
            timestamp,
            from_asset.contract_id(),
            to_asset.contract_id()
        );

        // Set deadline to 5 minutes from now
        let deadline_ms = timestamp + 300_000;

        let message = IntentMessage {
            intent_id,
            action: IntentAction::Swap {
                from_asset: AssetSpec {
                    defuse_asset_id: Self::to_defuse_asset_id(from_asset),
                    amount: Some(input_amount.0.to_string()),
                    min_amount: None,
                },
                to_asset: AssetSpec {
                    defuse_asset_id: Self::to_defuse_asset_id(to_asset),
                    amount: None,
                    min_amount: Some(min_output_amount.0.to_string()),
                },
            },
            deadline_ms,
            // Allow any solver to fulfill this intent
            solver_whitelist: None,
        };

        serde_json::to_string(&message).map_err(|e| {
            AppError::SerializationError(format!("Failed to create intent message: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl SwapProvider for IntentsSwap {
    #[instrument(skip(self), level = "debug", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        output_amount = %output_amount.0
    ))]
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        let input_amount = self
            .request_quote_from_solver(from_asset, to_asset, output_amount)
            .await?;

        debug!(
            input_amount = %input_amount.0,
            output_amount = %output_amount.0,
            "NEAR Intents quote received"
        );

        Ok(input_amount)
    }

    #[instrument(skip(self), level = "info", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        amount = %amount.0
    ))]
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: U128,
    ) -> AppResult<FinalExecutionStatus> {
        // Calculate minimum output with slippage tolerance
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let slippage_multiplier = 1.0 - (f64::from(self.max_slippage_bps) / 10000.0);

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let min_output_amount = U128((amount.0 as f64 * slippage_multiplier) as u128);

        // Create intent message
        let intent_msg =
            Self::create_intent_message(from_asset, to_asset, amount, min_output_amount)?;

        // Get transaction parameters
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        // Create transaction to submit intent
        // Note: The actual implementation would use ft_transfer_call or mt_transfer_call
        // to transfer tokens to the intents contract with the intent message
        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(
                from_asset.transfer_call_action(&self.intents_contract, amount.into(), &intent_msg),
            ))],
        });

        let status = send_tx(&self.client, &self.signer, Self::DEFAULT_TIMEOUT, tx)
            .await
            .map_err(AppError::from)?;

        debug!("NEAR Intents swap submitted successfully");

        Ok(status)
    }

    fn provider_name(&self) -> &'static str {
        "NEAR Intents"
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        // NEAR Intents supports both NEP-141 and NEP-245
        // In theory, it also supports cross-chain assets, but for this implementation
        // we'll focus on NEAR-native assets
        let from_supported = from_asset.clone().into_nep141().is_some()
            || from_asset.clone().into_nep245().is_some();
        let to_supported =
            to_asset.clone().into_nep141().is_some() || to_asset.clone().into_nep245().is_some();

        from_supported && to_supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{InMemorySigner, SecretKey};
    use templar_common::asset::BorrowAsset;

    #[test]
    fn test_defuse_asset_id_conversion() {
        // NEP-141
        let nep141: FungibleAsset<BorrowAsset> = "nep141:usdc.near".parse().unwrap();
        assert_eq!(IntentsSwap::to_defuse_asset_id(&nep141), "near:usdc.near");

        // NEP-245
        let nep245: FungibleAsset<BorrowAsset> = "nep245:multi.near:eth".parse().unwrap();
        assert_eq!(
            IntentsSwap::to_defuse_asset_id(&nep245),
            "near:multi.near/eth"
        );
    }

    #[test]
    fn test_intents_swap_creation() {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test");
        let signer = Arc::new(InMemorySigner::from_secret_key(
            "liquidator.testnet".parse().unwrap(),
            signer_key,
        ));

        let intents = IntentsSwap::new(client, signer, Network::Testnet);

        assert_eq!(intents.provider_name(), "NEAR Intents");
        assert_eq!(intents.intents_contract.as_str(), "intents.testnet");
        assert_eq!(
            intents.quote_timeout_ms,
            IntentsSwap::DEFAULT_QUOTE_TIMEOUT_MS
        );
    }

    #[test]
    fn test_supports_assets() {
        let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
        let signer_key = SecretKey::from_seed(near_crypto::KeyType::ED25519, "test");
        let signer = Arc::new(InMemorySigner::from_secret_key(
            "liquidator.testnet".parse().unwrap(),
            signer_key,
        ));

        let intents = IntentsSwap::new(client, signer, Network::Testnet);

        let nep141: FungibleAsset<BorrowAsset> = "nep141:token.near".parse().unwrap();
        let nep245: FungibleAsset<BorrowAsset> = "nep245:multi.near:token1".parse().unwrap();

        // Should support both NEP-141 and NEP-245
        assert!(intents.supports_assets(&nep141, &nep141));
        assert!(intents.supports_assets(&nep141, &nep245));
        assert!(intents.supports_assets(&nep245, &nep141));
        assert!(intents.supports_assets(&nep245, &nep245));
    }

    #[test]
    fn test_network_contract_selection() {
        assert_eq!(
            IntentsSwap::intents_contract_for_network(Network::Mainnet).as_str(),
            "intents.near"
        );
        assert_eq!(
            IntentsSwap::intents_contract_for_network(Network::Testnet).as_str(),
            "intents.testnet"
        );
    }
}
