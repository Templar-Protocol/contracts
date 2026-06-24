//! 1-Click API swap provider for NEAR Intents.
//!
//! Provides swap functionality using the 1-Click API, which simplifies
//! NEAR Intents cross-chain swaps through a REST interface.
//!
//! ## Supported Asset Types
//!
//! - **NEP-245 (NEAR Intents)**: Cross-chain assets wrapped in `intents.near`
//! - **NEP-141 on NEAR**: Direct NEAR tokens (automatically wrapped/unwrapped via Intents)
//!
//! The provider automatically detects asset types and configures the appropriate
//! deposit and recipient modes to deliver tokens in the correct format.
//!
//! ## Three-phase Swap Process
//!
//! 1. **Quote**: Request quote and receive deposit address
//! 2. **Deposit**: Transfer tokens to deposit address
//! 3. **Poll**: Monitor swap status until completion

use std::fmt::Write;

use near_account_id::AccountType;
use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
    AccountId,
};

use templar_common::asset::{AssetClass, FungibleAsset, FungibleAssetAmount};
use templar_gateway_client::SigningClient;
use templar_gateway_methods_spec::{storage, token, tx};
use templar_gateway_types::{
    CryptoHash, NearToken, OperationRecord, OperationStatus, StepStatus, U128 as GatewayU128,
};

use crate::rpc::{AppError, AppResult};
use crate::swap::SwapProvider;

/// 1-Click API base URL
const ONECLICK_API_BASE: &str = "https://1click.chaindefuser.com";

/// Default maximum slippage in basis points (3% = 300 bps)
pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 300;

/// Default transaction timeout in seconds
const DEFAULT_TIMEOUT: u64 = 120;

/// Polling interval for swap status checks in seconds
const POLL_INTERVAL_SECONDS: u64 = 10;

/// Maximum time to wait for swap completion in seconds (4 minutes)
const MAX_SWAP_WAIT_SECONDS: u64 = 240;

/// Swap type for the 1-Click API
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwapType {
    /// Exact input amount, variable output
    ExactInput,
    /// Exact output amount, variable input
    ExactOutput,
    /// Flexible input amount
    FlexInput,
    /// Any input amount
    AnyInput,
}

/// Quote request for the 1-Click API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteRequest {
    /// If true, simulates quote without generating deposit address
    dry: bool,
    /// Deposit mode: SIMPLE or MEMO
    deposit_mode: String,
    /// Type of swap
    swap_type: SwapType,
    /// Slippage tolerance in basis points
    slippage_tolerance: u32,
    /// Origin asset ID (format: `nep141:CONTRACT_ID`)
    origin_asset: String,
    /// Deposit type: `ORIGIN_CHAIN`
    deposit_type: String,
    /// Destination asset ID (format: `nep141:CONTRACT_ID`)
    destination_asset: String,
    /// Amount in smallest unit
    amount: String,
    /// Refund address
    refund_to: String,
    /// Refund type: `ORIGIN_CHAIN`
    refund_type: String,
    /// Recipient address
    recipient: String,
    /// Recipient type: `DESTINATION_CHAIN`
    recipient_type: String,
    /// Deadline as ISO timestamp
    deadline: String,
    /// Referral identifier (optional, lowercase only)
    #[serde(skip_serializing_if = "Option::is_none")]
    referral: Option<String>,
    /// Quote waiting time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    quote_waiting_time_ms: Option<u64>,
}

/// Quote details from the 1-Click API
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Quote {
    /// Address to deposit tokens to
    deposit_address: String,
    /// Optional memo for deposit
    deposit_memo: Option<String>,
    /// Actual input amount (may differ from requested)
    amount_in: String,
    /// Formatted input amount
    #[allow(dead_code)]
    amount_in_formatted: String,
    /// Input amount in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    amount_in_usd: Option<String>,
    /// Minimum input amount
    #[allow(dead_code)]
    min_amount_in: String,
    /// Expected output amount
    amount_out: String,
    /// Formatted output amount
    #[allow(dead_code)]
    amount_out_formatted: String,
    /// Output amount in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    amount_out_usd: Option<String>,
    /// Minimum output amount
    #[allow(dead_code)]
    min_amount_out: String,
    /// Deadline for the swap
    #[allow(dead_code)]
    deadline: String,
    /// Time when quote becomes inactive
    #[allow(dead_code)]
    time_when_inactive: String,
    /// Estimated time in seconds
    time_estimate: u64,
}

/// Quote response from the 1-Click API
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct QuoteResponse {
    /// Timestamp of the quote
    #[allow(dead_code)]
    timestamp: String,
    /// Signature for verification
    #[allow(dead_code)]
    signature: String,
    /// The quote details
    quote: Quote,
}

/// Deposit submission request
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DepositSubmitRequest {
    /// Transaction hash of the deposit
    tx_hash: String,
    /// Deposit address from quote
    deposit_address: String,
    /// NEAR sender account (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    near_sender_account: Option<String>,
    /// Memo if required
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<String>,
}

/// Swap status from the 1-Click API
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SwapStatus {
    /// Waiting for deposit
    PendingDeposit,
    /// Deposit transaction detected but not yet confirmed
    KnownDepositTx,
    /// Deposit received, processing swap
    Processing,
    /// Swap completed successfully
    Success,
    /// Deposit amount was incomplete
    IncompleteDeposit,
    /// Swap was refunded
    Refunded,
    /// Swap failed
    Failed,
}

/// Status response from the 1-Click API
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    /// Current status
    status: SwapStatus,
    /// Last update timestamp (optional, can be null during early stages)
    #[allow(dead_code)]
    updated_at: Option<String>,
    /// Swap details (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    swap_details: Option<SwapDetails>,
}

/// Detailed swap information
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapDetails {
    /// Intent transaction hashes
    #[serde(default)]
    #[allow(dead_code)]
    intent_hashes: Vec<String>,
    /// NEAR transaction hashes
    #[serde(default)]
    #[allow(dead_code)]
    near_tx_hashes: Vec<String>,
    /// Actual input amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_in: Option<String>,
    /// Formatted input amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_in_formatted: Option<String>,
    /// USD value of input amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_in_usd: Option<String>,
    /// Actual output amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_out: Option<String>,
    /// Formatted output amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_out_formatted: Option<String>,
    /// USD value of output amount (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    amount_out_usd: Option<String>,
    /// Slippage in basis points (`null` during `PENDING_DEPOSIT`)
    #[allow(dead_code)]
    slippage: Option<i32>,
    /// Origin chain transaction hashes
    #[serde(default)]
    #[allow(dead_code)]
    origin_chain_tx_hashes: Vec<TxHashWithExplorer>,
    /// Destination chain transaction hashes
    #[serde(default)]
    #[allow(dead_code)]
    destination_chain_tx_hashes: Vec<TxHashWithExplorer>,
    /// Refunded amount if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    refunded_amount: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxHashWithExplorer {
    #[allow(dead_code)]
    hash: String,
    #[allow(dead_code)]
    explorer_url: String,
}

/// Response structure for `/v0/tokens` endpoint.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenInfo {
    asset_id: String,
}

/// 1-Click API swap provider
#[derive(Clone)]
pub struct OneClickSwap {
    /// Gateway client (also carries the bound signer)
    client: SigningClient,
    /// Maximum slippage in basis points
    max_slippage_bps: u32,
    /// Transaction timeout
    #[allow(dead_code)]
    timeout: u64,
    /// HTTP client for API calls
    http_client: reqwest::Client,
    /// Optional API token for fee reduction
    api_token: Option<String>,
    /// Cached set of 1-Click supported token `assetId` values
    supported_tokens: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
}

impl std::fmt::Debug for OneClickSwap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OneClickSwap")
            .field("max_slippage_bps", &self.max_slippage_bps)
            .field("timeout", &self.timeout)
            .field("api_token", &self.api_token.is_some())
            .finish_non_exhaustive()
    }
}

impl OneClickSwap {
    /// Creates a new 1-Click API swap provider.
    ///
    /// # Arguments
    ///
    /// * `client` - Gateway client for transaction submission (binds the signer)
    /// * `max_slippage_bps` - Maximum slippage in basis points (default: 300 = 3%)
    /// * `api_token` - Optional API token to avoid 0.1% fee
    pub fn new(
        client: SigningClient,
        max_slippage_bps: Option<u32>,
        api_token: Option<String>,
    ) -> Self {
        Self {
            client,
            max_slippage_bps: max_slippage_bps.unwrap_or(DEFAULT_MAX_SLIPPAGE_BPS),
            timeout: DEFAULT_TIMEOUT,
            http_client: reqwest::Client::new(),
            api_token,
            supported_tokens: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    /// The bot's account ID (the bound signer on the gateway client).
    fn our_account(&self) -> AccountId {
        self.client.account_id().0.clone()
    }

    /// Extracts the most-recent transaction hash from a completed operation
    /// record, if any step reached the chain.
    fn operation_tx_hash(operation: &OperationRecord) -> Option<CryptoHash> {
        operation
            .steps
            .iter()
            .rev()
            .find_map(|step| match step.status {
                StepStatus::Prepared { tx_hash }
                | StepStatus::Submitted { tx_hash }
                | StepStatus::Succeeded { tx_hash }
                | StepStatus::Failed { tx_hash } => Some(tx_hash),
                StepStatus::NotStarted => None,
            })
    }

    /// Fetches the list of supported tokens from the 1-Click API `/v0/tokens`
    /// endpoint and populates the local cache.
    ///
    /// Should be called during service initialization and periodically during
    /// registry refresh to keep the cache up to date.
    pub async fn load_supported_tokens(&self) {
        let url = format!("{ONECLICK_API_BASE}/v0/tokens");
        match self
            .http_client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) if !response.status().is_success() => {
                tracing::warn!(
                    status = %response.status(),
                    "1-Click /v0/tokens returned error status"
                );
            }
            Ok(response) => match response.json::<Vec<TokenInfo>>().await {
                Ok(tokens) => {
                    let mut cache = self
                        .supported_tokens
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    cache.clear();
                    for token in &tokens {
                        cache.insert(token.asset_id.clone());
                    }
                    tracing::info!(
                        token_count = cache.len(),
                        "1-Click supported tokens cache loaded"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "Failed to parse 1-Click /v0/tokens response"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "Failed to fetch 1-Click supported tokens"
                );
            }
        }
    }

    /// Converts a `FungibleAsset` to 1-Click asset ID format.
    ///
    /// 1-Click asset identifiers follow the format:
    /// - NEAR NEP-141: `nep141:<contract_id>`
    ///
    /// For NEP-245 tokens, we extract the underlying token ID.
    fn to_oneclick_asset_id<A: AssetClass>(asset: &FungibleAsset<A>) -> String {
        match asset.clone().into_nep141() {
            Some(contract_id) => format!("nep141:{contract_id}"),
            None => {
                // NEP-245: extract underlying asset
                if let Some((_, token_id)) = asset.clone().into_nep245() {
                    // Token ID should already be in format "nep141:..."
                    token_id.to_string()
                } else {
                    // Fallback
                    format!("nep141:{}", asset.contract_id())
                }
            }
        }
    }

    /// Requests a quote from the 1-Click API.
    #[tracing::instrument(skip(self), level = "debug")]
    #[allow(clippy::too_many_lines)]
    async fn request_quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        input_amount: FungibleAssetAmount<F>,
    ) -> AppResult<QuoteResponse> {
        let from_asset_id = Self::to_oneclick_asset_id(from_asset);
        let to_asset_id = Self::to_oneclick_asset_id(to_asset);
        let recipient = self.our_account().to_string();

        let from_str = from_asset.to_string();
        let to_str = to_asset.to_string();

        // Calculate deadline (30 minutes from now)
        let deadline = chrono::Utc::now() + chrono::Duration::minutes(30);
        let deadline_str = deadline.to_rfc3339();

        // Determine deposit and recipient types based on asset types
        // - If from_asset is NEP-245 from intents.near: deposit_type = "INTENTS"
        // - If from_asset is direct NEP-141 on NEAR: deposit_type = "ORIGIN_CHAIN"
        let deposit_type = if from_asset.clone().into_nep245().is_some()
            && from_asset.contract_id() == "intents.near"
        {
            "INTENTS"
        } else {
            "ORIGIN_CHAIN"
        };

        // - If to_asset is NEP-245 from intents.near: recipient_type = "INTENTS" (wrapped output)
        // - If to_asset is direct NEP-141 on NEAR: recipient_type = "DESTINATION_CHAIN" (unwrapped output)
        let recipient_type = if to_asset.clone().into_nep245().is_some()
            && to_asset.contract_id() == "intents.near"
        {
            "INTENTS"
        } else {
            "DESTINATION_CHAIN"
        };

        let refund_type = deposit_type; // Refunds go back to where we deposited from

        let request = QuoteRequest {
            dry: false, // We want a real quote with deposit address
            deposit_mode: "SIMPLE".to_string(),
            // For post-liquidation swaps, we use EXACT_INPUT because we're swapping
            // the collateral we HAVE (received from liquidation), not requesting a specific output.
            // EXACT_INPUT: we specify exact amount we want to swap, API tells us how much we'll receive
            swap_type: SwapType::ExactInput,
            slippage_tolerance: self.max_slippage_bps,
            origin_asset: from_asset_id.clone(),
            deposit_type: deposit_type.to_string(),
            destination_asset: to_asset_id.clone(),
            amount: u128::from(input_amount).to_string(), // Input amount we're swapping
            refund_to: recipient.clone(),
            refund_type: refund_type.to_string(),
            recipient: recipient.clone(),
            recipient_type: recipient_type.to_string(),
            deadline: deadline_str,
            referral: Some("templar-liquidator".to_string()), // Track bot usage
            quote_waiting_time_ms: Some(5000),                // Wait up to 5 seconds for quote
        };

        let url = format!("{ONECLICK_API_BASE}/v0/quote");

        tracing::info!(
            from = %from_str,
            to = %to_str,
            amount_raw = %u128::from(input_amount),
            deposit_type = %deposit_type,
            recipient_type = %recipient_type,
            "Requesting quote from 1-Click API"
        );

        let mut req = self.http_client.post(&url).json(&request);

        // Add API token if available
        if let Some(token) = &self.api_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.map_err(|e| {
            tracing::error!(?e, "Failed to send quote request");
            crate::swap::SwapError::new(
                crate::swap::SwapErrorKind::NetworkError {
                    message: e.to_string(),
                },
                "Quote request",
            )
        })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            tracing::error!(?e, "Failed to read response");
            AppError::ValidationError(format!("Failed to read response: {e}"))
        })?;

        if !status.is_success() {
            let kind =
                crate::swap::SwapErrorKind::from_oneclick_response(status.as_u16(), &response_text);
            tracing::error!(
                status = %status,
                response = %response_text,
                retryable = kind.is_retryable(),
                "Quote request failed"
            );
            return Err(crate::swap::SwapError::new(kind, "Quote request").into());
        }

        let quote_response: QuoteResponse = near_sdk::serde_json::from_str(&response_text)
            .map_err(|e| {
                tracing::error!(?e, response = %response_text, "Failed to parse quote response");
                AppError::ValidationError(format!("Invalid quote response: {e}"))
            })?;

        let amount_out_u128: u128 = quote_response.quote.amount_out.parse().unwrap_or_default();
        let min_amount_out_u128: u128 = quote_response
            .quote
            .min_amount_out
            .parse()
            .unwrap_or_default();

        // Calculate exchange rate for logging
        let amount_in_u128: u128 = quote_response.quote.amount_in.parse().unwrap_or_default();
        #[allow(clippy::cast_precision_loss)]
        let exchange_rate = if amount_in_u128 > 0 {
            (amount_out_u128 as f64) / (amount_in_u128 as f64)
        } else {
            0.0
        };

        tracing::info!(
            deposit_address = %quote_response.quote.deposit_address,
            deposit_memo = ?quote_response.quote.deposit_memo,
            origin_asset_id = %from_asset_id,
            destination_asset_id = %to_asset_id,
            amount_in_raw = %amount_in_u128,
            amount_out_raw = %amount_out_u128,
            min_out_raw = %min_amount_out_u128,
            exchange_rate = %format!("{:.6}", exchange_rate),
            slippage_bps = %self.max_slippage_bps,
            time_estimate_s = %quote_response.quote.time_estimate,
            deadline = %quote_response.quote.deadline,
            quote_timestamp = %quote_response.timestamp,
            "Quote received from 1-Click API"
        );

        Ok(quote_response)
    }

    /// Registers storage for an account in a NEP-141 token contract.
    async fn ensure_storage_deposit<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        const MAX_REASONABLE_DEPOSIT: u128 = 100_000_000_000_000_000_000_000; // 0.1 NEAR

        tracing::debug!(
            token = %token_contract.contract_id(),
            account = %account_id,
            "Registering storage deposit for account"
        );

        // Query storage_balance_bounds to get minimum deposit required
        let bounds = self
            .client
            .read(storage::GetBalanceBounds {
                contract_id: token_contract.contract_id().into(),
            })
            .await
            .map_err(|e| {
                tracing::error!(?e, token = %token_contract.contract_id(), "Failed to query storage_balance_bounds");
                AppError::Rpc(e.into())
            })?;

        let min_deposit: NearToken = bounds.bounds.min;

        // Validate minimum deposit is reasonable (less than 0.1 NEAR)
        if min_deposit.as_yoctonear() > MAX_REASONABLE_DEPOSIT {
            return Err(AppError::ValidationError(format!(
                "Storage deposit minimum ({} yoctoNEAR) exceeds reasonable limit ({MAX_REASONABLE_DEPOSIT} yoctoNEAR / 0.1 NEAR)",
                min_deposit.as_yoctonear()
            )));
        }

        #[allow(clippy::cast_precision_loss)]
        let min_deposit_near = min_deposit.as_yoctonear() as f64 / 1e24;

        tracing::debug!(
            min_deposit_near = %min_deposit_near,
            "Storage deposit minimum from contract"
        );

        // Storage deposit can fail (already registered, or NEP-245 contracts that
        // handle storage internally). Both are non-fatal — we proceed regardless
        // of the resulting operation status, and only surface submission errors.
        match self
            .client
            .execute(storage::Deposit {
                contract_id: token_contract.contract_id().into(),
                beneficiary_id: Some(account_id.clone()),
                registration_only: true,
                deposit: min_deposit,
            })
            .await
        {
            Ok(result) => {
                tracing::debug!(
                    account = %account_id,
                    status = ?result.operation.status,
                    "Storage deposit completed"
                );
                Ok(())
            }
            Err(e) => Err(AppError::Rpc(e.into())),
        }
    }

    /// Deposits tokens to the 1-Click deposit address.
    #[allow(clippy::too_many_lines)]
    async fn deposit_tokens<F: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        deposit_address: &str,
        amount: U128,
        memo: Option<&str>,
    ) -> AppResult<String> {
        let asset_str = from_asset.to_string();

        tracing::info!(
            asset = %asset_str,
            amount_raw = %amount.0,
            deposit_address = %deposit_address,
            deposit_memo = ?memo,
            "Depositing tokens to 1-Click"
        );

        // Parse deposit address as NEAR account ID
        let deposit_account: AccountId = deposit_address.parse().map_err(|e| {
            tracing::error!(?e, deposit_address = %deposit_address, "Invalid deposit address");
            AppError::ValidationError(format!("Invalid deposit address: {e}"))
        })?;

        match deposit_account.get_account_type() {
            AccountType::NearDeterministicAccount => {
                tracing::error!(
                    deposit_account = %deposit_account,
                    deposit_address = %deposit_address,
                    "Deterministic 1-Click deposit addresses are not supported"
                );
                return Err(AppError::ValidationError(
                    "Deterministic 1-Click deposit addresses are not supported".to_string(),
                ));
            }
            // For implicit accounts, we need to ensure they exist first
            // by sending a small amount of NEAR to create the account.
            AccountType::NearImplicitAccount => {
                tracing::debug!(
                    deposit_account = %deposit_account,
                    "Creating implicit account"
                );

                // Send 1 yoctoNEAR to create the implicit account (minimum amount needed).
                // Don't fail if the account already exists.
                match self
                    .client
                    .execute(tx::Transfer {
                        receiver_id: deposit_account.clone(),
                        amount: NearToken::from_yoctonear(1),
                    })
                    .await
                {
                    Ok(_) => {
                        tracing::debug!(deposit_account = %deposit_account, "Implicit account created");

                        // Wait for account creation to propagate (1-2 blocks)
                        // This prevents race conditions with storage registration
                        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
                    }
                    Err(e) => {
                        // If account already exists, that's fine
                        tracing::warn!(
                            deposit_account = %deposit_account,
                            error = %e,
                            "Failed to create implicit account (may already exist)"
                        );
                    }
                }
            }
            _ => {}
        }

        // Ensure the deposit address is registered for storage
        // Skip for NEP-245 tokens (they handle storage internally)
        if from_asset.clone().into_nep141().is_some() {
            self.ensure_storage_deposit(from_asset, &deposit_account)
                .await?;
        } else {
            tracing::debug!(
                token = %from_asset.contract_id(),
                "Skipping storage_deposit for NEP-245 token (handles storage internally)"
            );
        }

        // Create deposit transaction.
        // Use a simple transfer (not transfer_call) for INTENTS depositType
        // because the implicit account doesn't have a contract to handle callbacks.
        // `token::Transfer` is standard-agnostic so NEP-245 collateral works too.
        let operation_result = self
            .client
            .execute(token::Transfer {
                token: token::TokenReference::from(from_asset),
                receiver_id: deposit_account.clone(),
                amount: GatewayU128(amount.0),
                memo: None,
            })
            .await
            .map_err(|e| AppError::Rpc(e.into()))?;

        let operation = &operation_result.operation;
        let tx_hash = Self::operation_tx_hash(operation);
        let tx_hash_str = tx_hash.map_or_else(|| operation.id.0.clone(), |hash| hash.to_string());

        match operation.status {
            OperationStatus::Succeeded => {
                let account_type_str = match deposit_account.get_account_type() {
                    AccountType::NamedAccount => "named",
                    AccountType::NearImplicitAccount => "implicit",
                    AccountType::EthImplicitAccount => "eth-implicit",
                    AccountType::NearDeterministicAccount => "deterministic",
                };
                tracing::info!(
                    tx_hash = %tx_hash_str,
                    asset = %asset_str,
                    amount_raw = %amount.0,
                    deposit_address = %deposit_address,
                    account_type = %account_type_str,
                    "Deposit transaction succeeded"
                );
            }
            failed_status => {
                tracing::error!(
                    tx_hash = %tx_hash_str,
                    status = ?failed_status,
                    "Deposit transaction failed"
                );
                return Err(AppError::ValidationError(format!(
                    "Deposit transaction failed: operation {} ended with status {failed_status:?}",
                    operation.id.0
                )));
            }
        }

        // Check if the deposit was refunded by fetching the transaction logs.
        let refunded = match tx_hash {
            Some(hash) => {
                self.check_deposit_refunded(hash, &deposit_account, amount)
                    .await
            }
            None => Ok(None),
        };

        match refunded {
            Ok(Some(refund_amount)) => {
                tracing::error!(
                    tx_hash = %tx_hash_str,
                    deposit_account = %deposit_account,
                    refund_amount = %refund_amount.0,
                    "Deposit was refunded - 1-Click rejected the deposit"
                );
                return Err(AppError::ValidationError(format!(
                    "Deposit was refunded by 1-Click deposit address (amount: {})",
                    refund_amount.0
                )));
            }
            Ok(None) => {
                tracing::debug!(tx_hash = %tx_hash_str, "Deposit accepted");
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    "Failed to check if deposit was refunded, assuming accepted"
                );
            }
        }

        Ok(tx_hash_str)
    }

    /// Checks if a deposit was refunded by examining the transaction logs.
    ///
    /// Returns the amount refunded if the deposit was rejected, or None if successful.
    async fn check_deposit_refunded(
        &self,
        tx_hash: CryptoHash,
        deposit_account: &AccountId,
        _amount: U128,
    ) -> AppResult<Option<U128>> {
        let our_account = self.our_account();

        // Fetch the transaction (aggregated receipt logs) through the gateway.
        let tx_result = self
            .client
            .read(tx::Get {
                tx_hash,
                sender_account_id: our_account.clone(),
                wait_until: Some(templar_gateway_types::common::TxExecutionStatus::Final),
                encoding: tx::ValueEncoding::default(),
            })
            .await
            .map_err(|e| AppError::Rpc(e.into()))?;

        // Check transfer events. If we see a transfer TO deposit_account followed
        // by a transfer FROM deposit_account back to us, extract the refund amount.
        let mut tokens_sent = false;
        let mut refund_amount: Option<U128> = None;

        for log in &tx_result.logs {
            // Check for NEP-141 transfer events
            if log.contains("EVENT_JSON") && log.contains("ft_transfer") {
                // Parse the event to check direction and extract amount
                if log.contains(&format!("\"new_owner_id\":\"{deposit_account}\"")) {
                    tokens_sent = true;
                }
                if log.contains(&format!("\"old_owner_id\":\"{deposit_account}\""))
                    && log.contains(&format!("\"new_owner_id\":\"{our_account}\""))
                {
                    // Extract amount from the event JSON
                    // Format: EVENT_JSON:{"standard":"nep141",...,"data":[{"amount":"..."}]}
                    if let Some(amount_str) = Self::extract_transfer_amount(log) {
                        if let Ok(amount_value) = amount_str.parse::<u128>() {
                            refund_amount = Some(U128(amount_value));
                        }
                    }
                }
            }
        }

        // Return refund amount if both sent and returned
        if tokens_sent && refund_amount.is_some() {
            Ok(refund_amount)
        } else {
            Ok(None)
        }
    }

    /// Extracts the transfer amount from a NEP-141 `EVENT_JSON` log entry.
    fn extract_transfer_amount(log: &str) -> Option<String> {
        // Format: EVENT_JSON:{"standard":"nep141",...,"data":[{"amount":"12345",...}]}
        // Find the "amount" field value
        if let Some(amount_start) = log.find(r#""amount":""#) {
            let amount_start = amount_start + r#""amount":""#.len();
            if let Some(amount_end) = log[amount_start..].find('"') {
                return Some(log[amount_start..amount_start + amount_end].to_string());
            }
        }
        None
    }

    /// Notifies 1-Click API of the deposit.
    async fn submit_deposit(
        &self,
        tx_hash: &str,
        deposit_address: &str,
        memo: Option<&str>,
    ) -> AppResult<()> {
        let request = DepositSubmitRequest {
            tx_hash: tx_hash.to_string(),
            deposit_address: deposit_address.to_string(),
            near_sender_account: Some(self.our_account().to_string()),
            memo: memo.map(String::from),
        };

        let url = format!("{ONECLICK_API_BASE}/v0/deposit/submit");
        let mut req = self.http_client.post(&url).json(&request);

        if let Some(token) = &self.api_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.map_err(|e| {
            tracing::error!(?e, "Failed to submit deposit");
            AppError::ValidationError(format!("Deposit submit failed: {e}"))
        })?;

        if !response.status().is_success() {
            use reqwest::StatusCode;
            let status = response.status();
            let response_text = response.text().await.unwrap_or_default();
            let error_msg = match status {
                StatusCode::BAD_REQUEST => {
                    format!("Bad Request - Invalid deposit data: {response_text}")
                }
                StatusCode::UNAUTHORIZED => {
                    format!("Unauthorized - JWT token is invalid: {response_text}")
                }
                StatusCode::NOT_FOUND => {
                    format!("Not Found - Deposit address not found: {response_text}")
                }
                _ => format!("Deposit submission failed with status {status}: {response_text}"),
            };
            tracing::error!(
                status = %status,
                response = %response_text,
                "Deposit submission failed"
            );
            return Err(AppError::ValidationError(error_msg));
        }

        tracing::info!(
            tx_hash = %tx_hash,
            deposit_address = %deposit_address,
            near_sender = %self.our_account(),
            memo = ?memo,
            "Deposit submitted to 1-Click API"
        );
        Ok(())
    }

    /// Polls the swap status until completion.
    #[allow(clippy::too_many_lines)]
    async fn poll_swap_status(
        &self,
        deposit_address: &str,
        memo: Option<&str>,
        max_wait_seconds: u64,
    ) -> AppResult<SwapStatus> {
        let max_attempts = max_wait_seconds / POLL_INTERVAL_SECONDS;

        tracing::debug!(
            max_wait_s = %max_wait_seconds,
            "Polling swap status"
        );

        for attempt in 1..=max_attempts {
            tokio::time::sleep(tokio::time::Duration::from_secs(POLL_INTERVAL_SECONDS)).await;

            let mut url = format!("{ONECLICK_API_BASE}/v0/status?depositAddress={deposit_address}");
            if let Some(m) = memo {
                let _ = write!(url, "&depositMemo={m}");
            }

            let mut req = self.http_client.get(&url);
            if let Some(token) = &self.api_token {
                req = req.bearer_auth(token);
            }

            let response = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(?e, attempt = %attempt, "Failed to fetch status");
                    continue;
                }
            };

            if !response.status().is_success() {
                use reqwest::StatusCode;
                let status_code = response.status();
                let error_text = response.text().await.unwrap_or_default();
                match status_code {
                    StatusCode::UNAUTHORIZED => tracing::warn!(
                        attempt = %attempt,
                        "Unauthorized - JWT token may be invalid"
                    ),
                    StatusCode::NOT_FOUND => tracing::warn!(
                        attempt = %attempt,
                        deposit_address = %deposit_address,
                        "Deposit address not found - swap may not have been initiated yet"
                    ),
                    _ => tracing::warn!(
                        status = %status_code,
                        attempt = %attempt,
                        error = %error_text,
                        "Status request failed"
                    ),
                }
                continue;
            }

            // Get raw response text for debugging
            let response_text = match response.text().await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(?e, attempt = %attempt, "Failed to read status response text");
                    continue;
                }
            };

            tracing::debug!(response = %response_text, "Raw status response");

            let status_response: StatusResponse = match near_sdk::serde_json::from_str(
                &response_text,
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?e, response = %response_text, attempt = %attempt, "Failed to parse status response");
                    continue;
                }
            };

            tracing::debug!(
                attempt = %attempt,
                status = ?status_response.status,
                "Swap status"
            );

            match status_response.status {
                SwapStatus::Success => {
                    // Log comprehensive swap completion details
                    if let Some(ref details) = status_response.swap_details {
                        tracing::info!(
                            deposit_address = %deposit_address,
                            status = "SUCCESS",
                            amount_in = ?details.amount_in,
                            amount_in_formatted = ?details.amount_in_formatted,
                            amount_in_usd = ?details.amount_in_usd,
                            amount_out = ?details.amount_out,
                            amount_out_formatted = ?details.amount_out_formatted,
                            amount_out_usd = ?details.amount_out_usd,
                            slippage_bps = ?details.slippage,
                            intent_hashes = ?details.intent_hashes,
                            near_tx_hashes = ?details.near_tx_hashes,
                            origin_chain_txs = ?details.origin_chain_tx_hashes.iter().map(|tx| &tx.hash).collect::<Vec<_>>(),
                            destination_chain_txs = ?details.destination_chain_tx_hashes.iter().map(|tx| &tx.hash).collect::<Vec<_>>(),
                            attempt = %attempt,
                            "Swap completed successfully via 1-Click"
                        );
                    } else {
                        tracing::info!(
                            deposit_address = %deposit_address,
                            status = "SUCCESS",
                            attempt = %attempt,
                            "Swap completed successfully (no details available)"
                        );
                    }
                    return Ok(SwapStatus::Success);
                }
                SwapStatus::Failed | SwapStatus::Refunded => {
                    // Log detailed failure information
                    if let Some(ref details) = status_response.swap_details {
                        tracing::error!(
                            deposit_address = %deposit_address,
                            status = ?status_response.status,
                            refunded_amount = ?details.refunded_amount,
                            amount_in = ?details.amount_in,
                            amount_out = ?details.amount_out,
                            intent_hashes = ?details.intent_hashes,
                            near_tx_hashes = ?details.near_tx_hashes,
                            origin_chain_txs = ?details.origin_chain_tx_hashes.iter().map(|tx| &tx.explorer_url).collect::<Vec<_>>(),
                            destination_chain_txs = ?details.destination_chain_tx_hashes.iter().map(|tx| &tx.explorer_url).collect::<Vec<_>>(),
                            attempt = %attempt,
                            "Swap failed or refunded - contact 1-Click support with these details"
                        );
                    } else {
                        tracing::error!(
                            deposit_address = %deposit_address,
                            status = ?status_response.status,
                            attempt = %attempt,
                            "Swap failed or refunded (no details available)"
                        );
                    }
                    return Ok(status_response.status);
                }
                SwapStatus::PendingDeposit
                | SwapStatus::KnownDepositTx
                | SwapStatus::Processing => {
                    tracing::debug!(status = ?status_response.status, "Swap still in progress");
                    // Continue polling
                }
                SwapStatus::IncompleteDeposit => {
                    if let Some(ref details) = status_response.swap_details {
                        tracing::warn!(
                            deposit_address = %deposit_address,
                            status = "INCOMPLETE_DEPOSIT",
                            amount_in = ?details.amount_in,
                            amount_out = ?details.amount_out,
                            refunded_amount = ?details.refunded_amount,
                            attempt = %attempt,
                            "Incomplete deposit detected - partial amount deposited"
                        );
                    } else {
                        tracing::warn!(
                            deposit_address = %deposit_address,
                            status = "INCOMPLETE_DEPOSIT",
                            attempt = %attempt,
                            "Incomplete deposit detected"
                        );
                    }
                    return Ok(SwapStatus::IncompleteDeposit);
                }
            }
        }

        tracing::warn!("Swap status polling timed out");
        Err(crate::swap::SwapError::new(
            crate::swap::SwapErrorKind::Timeout {
                message: format!("Swap did not complete within {max_wait_seconds}s"),
            },
            "Poll swap status",
        )
        .into())
    }
}

#[async_trait::async_trait]
impl SwapProvider for OneClickSwap {
    #[tracing::instrument(skip(self), level = "debug", fields(
        provider = %self.provider_name(),
        from = %from_asset.to_string(),
        to = %to_asset.to_string(),
        output_amount = %output_amount
    ))]
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: FungibleAssetAmount<T>,
    ) -> AppResult<FungibleAssetAmount<F>> {
        // Silence unused warnings - parameters needed for tracing
        let _ = (from_asset, to_asset, output_amount);

        // OneClick uses EXACT_INPUT mode, so output-based quotes are not supported.
        // For the rebalancer use case, call swap() directly with the input amount.
        Err(AppError::ValidationError(
            "OneClick provider only supports EXACT_INPUT swaps. Use swap() directly with input amount.".to_string()
        ))
    }

    #[tracing::instrument(skip(self, from_asset, to_asset), level = "info")]
    async fn swap<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        amount: FungibleAssetAmount<F>,
    ) -> AppResult<()> {
        let swap_start = std::time::Instant::now();

        tracing::info!(
            from_asset = %from_asset.to_string(),
            to_asset = %to_asset.to_string(),
            amount_raw = %u128::from(amount),
            "Starting 1-Click swap"
        );

        // Step 1: Get quote with deposit address
        let quote_response = self.request_quote(from_asset, to_asset, amount).await?;

        let deposit_address = &quote_response.quote.deposit_address;
        let memo = quote_response.quote.deposit_memo.as_deref();
        let input_amount_str = &quote_response.quote.amount_in;

        let input_amount: u128 = input_amount_str.parse().map_err(|e| {
            tracing::error!(?e, amount = %input_amount_str, "Failed to parse input amount");
            AppError::ValidationError(format!("Invalid input amount: {e}"))
        })?;

        // Step 2: Deposit tokens
        let tx_hash = self
            .deposit_tokens(from_asset, deposit_address, U128(input_amount), memo)
            .await?;

        // Step 3: Notify 1-Click of deposit
        self.submit_deposit(&tx_hash, deposit_address, memo).await?;

        // Step 4: Poll for completion
        let status = self
            .poll_swap_status(deposit_address, memo, MAX_SWAP_WAIT_SECONDS)
            .await?;

        let swap_duration = swap_start.elapsed();

        if status == SwapStatus::Success {
            tracing::info!(
                deposit_address = %deposit_address,
                deposit_tx_hash = %tx_hash,
                duration_ms = swap_duration.as_millis(),
                "1-Click swap completed successfully"
            );
            Ok(())
        } else {
            tracing::error!(
                deposit_address = %deposit_address,
                deposit_tx_hash = %tx_hash,
                status = ?status,
                duration_ms = swap_duration.as_millis(),
                "1-Click swap did not succeed"
            );
            Err(crate::swap::SwapError::new(
                crate::swap::SwapErrorKind::Unknown {
                    message: format!("Swap ended with status: {status:?}"),
                },
                "1-Click swap",
            )
            .into())
        }
    }

    fn provider_name(&self) -> &'static str {
        "1-Click API (NEAR Intents)"
    }

    fn supports_assets<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
    ) -> bool {
        // 1-Click API supports:
        // 1. NEP-245 (NEAR Intents) tokens - cross-chain assets via intents.near
        // 2. Direct NEP-141 tokens on NEAR (can be wrapped/unwrapped to/from Intents)
        //
        // At least ONE asset should be NEP-245 (Intents) for 1-Click to be useful.
        // If both are direct NEP-141 on NEAR, other DEXes (like Ref) would be better.
        let from_is_nep245 = from_asset.clone().into_nep245().is_some();
        let to_is_nep245 = to_asset.clone().into_nep245().is_some();

        if !(from_is_nep245 || to_is_nep245) {
            return false;
        }

        // Check against the cached supported tokens list from /v0/tokens.
        // If the cache is empty (not yet loaded), allow the swap to proceed —
        // a quote failure will surface the issue at runtime.
        let cache = self
            .supported_tokens
            .read()
            .unwrap_or_else(|e| e.into_inner());
        if cache.is_empty() {
            return true;
        }

        let from_id = Self::to_oneclick_asset_id(from_asset);
        let to_id = Self::to_oneclick_asset_id(to_asset);
        let from_ok = cache.contains(&from_id);
        let to_ok = cache.contains(&to_id);

        if !from_ok || !to_ok {
            tracing::debug!(
                from = %from_id,
                to = %to_id,
                from_supported = from_ok,
                to_supported = to_ok,
                "1-Click does not support one or both tokens"
            );
        }

        from_ok && to_ok
    }

    async fn ensure_storage_registration<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        // Delegate to the existing ensure_storage_deposit method
        self.ensure_storage_deposit(token_contract, account_id)
            .await
    }
}
