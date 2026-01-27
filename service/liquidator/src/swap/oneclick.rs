//! 1-Click API swap provider for NEAR Intents.
//!
//! Provides swap functionality using the 1-Click API, which simplifies
//! NEAR Intents cross-chain swaps through a REST interface.
//!
//! ## Three-phase process:
//! 1. **Quote**: Request quote and receive deposit address
//! 2. **Deposit**: Transfer tokens to deposit address
//! 3. **Poll**: Monitor swap status until completion

use std::{fmt::Write, sync::Arc};

use near_account_id::AccountType;
use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    types::AccountId,
    views::FinalExecutionStatus,
};
use near_sdk::{
    json_types::U128,
    serde::{Deserialize, Serialize},
};

use templar_common::asset::{AssetClass, FungibleAsset, FungibleAssetAmount};

use crate::rpc::{get_access_key_data, send_tx, view, AppError, AppResult};
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

/// Storage balance bounds from NEP-145
#[derive(Debug, Deserialize)]
struct StorageBalanceBounds {
    /// Minimum storage deposit required
    min: U128,
    /// Maximum storage deposit allowed (optional)
    #[allow(dead_code)]
    max: Option<U128>,
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

/// 1-Click API swap provider
#[derive(Debug, Clone)]
pub struct OneClickSwap {
    /// NEAR RPC client
    client: JsonRpcClient,
    /// Transaction signer
    signer: Arc<Signer>,
    /// Maximum slippage in basis points
    max_slippage_bps: u32,
    /// Transaction timeout
    timeout: u64,
    /// HTTP client for API calls
    http_client: reqwest::Client,
    /// Optional API token for fee reduction
    api_token: Option<String>,
}

impl OneClickSwap {
    /// Creates a new 1-Click API swap provider.
    ///
    /// # Arguments
    ///
    /// * `client` - NEAR RPC client for transaction submission
    /// * `signer` - Transaction signer
    /// * `max_slippage_bps` - Maximum slippage in basis points (default: 300 = 3%)
    /// * `api_token` - Optional API token to avoid 0.1% fee
    pub fn new(
        client: JsonRpcClient,
        signer: Arc<Signer>,
        max_slippage_bps: Option<u32>,
        api_token: Option<String>,
    ) -> Self {
        Self {
            client,
            signer,
            max_slippage_bps: max_slippage_bps.unwrap_or(DEFAULT_MAX_SLIPPAGE_BPS),
            timeout: DEFAULT_TIMEOUT,
            http_client: reqwest::Client::new(),
            api_token,
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
    async fn request_quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        input_amount: FungibleAssetAmount<F>,
    ) -> AppResult<QuoteResponse> {
        let from_asset_id = Self::to_oneclick_asset_id(from_asset);
        let to_asset_id = Self::to_oneclick_asset_id(to_asset);
        let recipient = self.signer.get_account_id().to_string();

        let from_str = from_asset.to_string();
        let to_str = to_asset.to_string();

        // Calculate deadline (30 minutes from now)
        let deadline = chrono::Utc::now() + chrono::Duration::minutes(30);
        let deadline_str = deadline.to_rfc3339();

        // For liquidation bot, we always deposit via NEAR Intents contract
        // since our bot runs on NEAR and holds NEP-141 tokens.
        // ORIGIN_CHAIN would be used if we were depositing from another blockchain (e.g., ETH on Ethereum)
        let deposit_type = "INTENTS";

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
            // INTENTS: refunds go back to our NEAR account within Intents contract
            refund_type: "INTENTS".to_string(),
            recipient: recipient.clone(),
            // INTENTS: swapped tokens delivered to our NEAR account within Intents contract
            recipient_type: "INTENTS".to_string(),
            deadline: deadline_str,
            referral: Some("templar-liquidator".to_string()), // Track bot usage
            quote_waiting_time_ms: Some(5000),                // Wait up to 5 seconds for quote
        };

        let url = format!("{ONECLICK_API_BASE}/v0/quote");

        tracing::info!(
            from = %from_str,
            to = %to_str,
            amount_raw = %u128::from(input_amount),
            "Requesting quote from 1-Click API"
        );

        let mut req = self.http_client.post(&url).json(&request);

        // Add API token if available
        if let Some(token) = &self.api_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.map_err(|e| {
            tracing::error!(?e, "Failed to send quote request");
            AppError::ValidationError(format!("Quote request failed: {e}"))
        })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            tracing::error!(?e, "Failed to read response");
            AppError::ValidationError(format!("Failed to read response: {e}"))
        })?;

        if !status.is_success() {
            use reqwest::StatusCode;
            let error_msg = match status {
                StatusCode::BAD_REQUEST => {
                    format!("Bad Request - Invalid input data: {response_text}")
                }
                StatusCode::UNAUTHORIZED => {
                    format!("Unauthorized - JWT token is invalid or missing: {response_text}")
                }
                StatusCode::NOT_FOUND => {
                    format!("Not Found - Endpoint or resource not found: {response_text}")
                }
                _ => format!("Quote request failed with status {status}: {response_text}"),
            };
            tracing::error!(
                status = %status,
                response = %response_text,
                "Quote request failed"
            );
            return Err(AppError::ValidationError(error_msg));
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

        tracing::info!(
            out_raw = %amount_out_u128,
            min_out_raw = %min_amount_out_u128,
            time_estimate_s = %quote_response.quote.time_estimate,
            "Quote received"
        );

        Ok(quote_response)
    }

    /// Registers storage for an account in a NEP-141 token contract.
    async fn ensure_storage_deposit<F: AssetClass>(
        &self,
        token_contract: &FungibleAsset<F>,
        account_id: &AccountId,
    ) -> AppResult<()> {
        use near_primitives::transaction::{Action, FunctionCallAction};
        use near_sdk::Gas;

        const MAX_REASONABLE_DEPOSIT: u128 = 100_000_000_000_000_000_000_000; // 0.1 NEAR

        tracing::debug!(
            token = %token_contract.contract_id(),
            account = %account_id,
            "Registering storage deposit for account"
        );

        // Query storage_balance_bounds to get minimum deposit required
        let bounds: StorageBalanceBounds = view(
            &self.client,
            token_contract.contract_id().into(),
            "storage_balance_bounds",
            near_sdk::serde_json::json!({}),
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, token = %token_contract.contract_id(), "Failed to query storage_balance_bounds");
            AppError::Rpc(e)
        })?;

        let min_deposit = bounds.min.0;

        // Validate minimum deposit is reasonable (less than 0.1 NEAR)
        if min_deposit > MAX_REASONABLE_DEPOSIT {
            return Err(AppError::ValidationError(format!(
                "Storage deposit minimum ({min_deposit} yoctoNEAR) exceeds reasonable limit ({MAX_REASONABLE_DEPOSIT} yoctoNEAR / 0.1 NEAR)"
            )));
        }

        #[allow(clippy::cast_precision_loss)]
        let min_deposit_near = min_deposit as f64 / 1e24;

        tracing::debug!(
            min_deposit_near = %min_deposit_near,
            "Storage deposit minimum from contract"
        );

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        let storage_deposit_action = FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: near_sdk::serde_json::to_vec(&near_sdk::serde_json::json!({
                "account_id": account_id,
                "registration_only": true,
            }))
            .map_err(|e| AppError::ValidationError(format!("Failed to serialize args: {e}")))?,
            gas: Gas::from_tgas(10).as_gas(),
            deposit: min_deposit,
        };

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: token_contract.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(storage_deposit_action))],
        });

        let outcome = send_tx(&self.client, &self.signer, self.timeout, tx).await?;

        match outcome.status {
            FinalExecutionStatus::SuccessValue(_) => {
                tracing::debug!(account = %account_id, "Storage deposit successful");
                Ok(())
            }
            FinalExecutionStatus::Failure(failure) => {
                // Storage deposit can fail if:
                // 1. Already registered (common)
                // 2. Contract doesn't support storage_deposit (NEP-245 multi-tokens)
                // Both cases are fine - we can proceed with the transfer
                tracing::debug!(
                    account = %account_id,
                    failure = ?failure,
                    "Storage deposit failed (likely already registered or not required)"
                );
                Ok(())
            }
            _ => {
                tracing::debug!(status = ?outcome.status, "Unexpected storage deposit status");
                Ok(())
            }
        }
    }

    /// Deposits tokens to the 1-Click deposit address.
    #[allow(clippy::too_many_lines)]
    async fn deposit_tokens<F: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        deposit_address: &str,
        amount: U128,
        _memo: Option<&str>,
    ) -> AppResult<String> {
        let asset_str = from_asset.to_string();

        tracing::info!(
            asset = %asset_str,
            amount_raw = %amount.0,
            deposit_address = %deposit_address,
            "Depositing tokens to 1-Click"
        );

        // Parse deposit address as NEAR account ID
        let deposit_account: AccountId = deposit_address.parse().map_err(|e| {
            tracing::error!(?e, deposit_address = %deposit_address, "Invalid deposit address");
            AppError::ValidationError(format!("Invalid deposit address: {e}"))
        })?;

        // For implicit accounts, we need to ensure they exist first
        // by sending a small amount of NEAR to create the account
        if deposit_account.get_account_type() == AccountType::NearImplicitAccount {
            tracing::debug!(
                deposit_account = %deposit_account,
                "Creating implicit account"
            );

            let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

            // Send 1 yoctoNEAR to create the implicit account (minimum amount needed)
            let create_account_tx = Transaction::V0(TransactionV0 {
                nonce,
                receiver_id: deposit_account.clone(),
                block_hash,
                signer_id: self.signer.get_account_id(),
                public_key: self.signer.public_key().clone(),
                actions: vec![Action::Transfer(near_primitives::action::TransferAction {
                    deposit: 1, // 1 yoctoNEAR
                })],
            });

            // Send transaction but don't fail if account already exists
            match send_tx(&self.client, &self.signer, self.timeout, create_account_tx).await {
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
                        error = ?e,
                        "Failed to create implicit account (may already exist)"
                    );
                }
            }
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

        // Get transaction parameters
        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        // Create deposit transaction
        // Use simple ft_transfer (not ft_transfer_call) for INTENTS depositType
        // because the implicit account doesn't have a contract to handle callbacks
        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: from_asset.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(
                from_asset.transfer_action(&deposit_account, amount.into()),
            ))],
        });

        // Get the transaction hash before sending
        let (tx_hash, _) = tx.get_hash_and_size();
        let tx_hash_str = tx_hash.to_string();

        let outcome = send_tx(&self.client, &self.signer, self.timeout, tx).await?;

        match &outcome.status {
            FinalExecutionStatus::SuccessValue(_) => {
                tracing::debug!("Deposit transaction succeeded");
            }
            FinalExecutionStatus::Failure(failure) => {
                tracing::error!(
                    failure = ?failure,
                    "Deposit transaction failed with detailed error"
                );
                return Err(AppError::ValidationError(format!(
                    "Deposit transaction failed: {failure:?}"
                )));
            }
            _ => {
                tracing::warn!(status = ?outcome.status, "Unexpected transaction status");
            }
        }

        // Check if the deposit was refunded by fetching transaction outcome and checking receipts
        match self
            .check_deposit_refunded(&tx_hash_str, &deposit_account, amount)
            .await
        {
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

    /// Checks if a deposit was refunded by examining transaction receipts.
    ///
    /// Returns the amount refunded if the deposit was rejected, or None if successful.
    async fn check_deposit_refunded(
        &self,
        tx_hash: &str,
        deposit_account: &AccountId,
        _amount: U128,
    ) -> AppResult<Option<U128>> {
        use near_jsonrpc_client::methods::tx::{RpcTransactionStatusRequest, TransactionInfo};
        use near_primitives::views::TxExecutionStatus;

        // Parse tx hash
        let tx_hash_parsed = tx_hash
            .parse()
            .map_err(|e| AppError::ValidationError(format!("Invalid tx hash: {e}")))?;

        // Fetch transaction outcome
        let tx_result = self
            .client
            .call(RpcTransactionStatusRequest {
                transaction_info: TransactionInfo::TransactionId {
                    sender_account_id: self.signer.get_account_id(),
                    tx_hash: tx_hash_parsed,
                },
                wait_until: TxExecutionStatus::Final,
            })
            .await
            .map_err(|e| AppError::Rpc(e.into()))?;

        // Check receipt outcomes for token transfers
        // If we see a transfer TO deposit_account followed by a transfer FROM deposit_account
        // back to us, extract the refund amount
        let mut tokens_sent = false;
        let mut refund_amount: Option<U128> = None;

        // Get receipts from the transaction result
        let receipts = match &tx_result.final_execution_outcome {
            Some(outcome) => {
                match outcome {
                    near_primitives::views::FinalExecutionOutcomeViewEnum::FinalExecutionOutcome(o) => {
                        &o.receipts_outcome
                    }
                    near_primitives::views::FinalExecutionOutcomeViewEnum::FinalExecutionOutcomeWithReceipt(_o) => {
                        // For this variant, we need to construct a vec with the single receipt
                        // Since we can't easily return different types, let's just return empty
                        // and check the transaction outcome logs instead
                        &Vec::new()
                    }
                }
            }
            None => {
                return Err(AppError::ValidationError(
                    "No execution outcome".to_string(),
                ))
            }
        };

        for receipt in receipts {
            for log in &receipt.outcome.logs {
                // Check for NEP-141 transfer events
                if log.contains("EVENT_JSON") && log.contains("ft_transfer") {
                    // Parse the event to check direction and extract amount
                    if log.contains(&format!("\"new_owner_id\":\"{deposit_account}\"")) {
                        tokens_sent = true;
                    }
                    if log.contains(&format!("\"old_owner_id\":\"{deposit_account}\""))
                        && log.contains(&format!(
                            "\"new_owner_id\":\"{}\"",
                            self.signer.get_account_id()
                        ))
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
            near_sender_account: Some(self.signer.get_account_id().to_string()),
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

        tracing::debug!(tx_hash = %tx_hash, "Deposit submitted to 1-Click API");
        Ok(())
    }

    /// Polls the swap status until completion.
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
                    tracing::debug!("Swap completed successfully");
                    return Ok(SwapStatus::Success);
                }
                SwapStatus::Failed | SwapStatus::Refunded => {
                    tracing::error!(status = ?status_response.status, "Swap failed or refunded");
                    return Ok(status_response.status);
                }
                SwapStatus::PendingDeposit
                | SwapStatus::KnownDepositTx
                | SwapStatus::Processing => {
                    tracing::debug!(status = ?status_response.status, "Swap still in progress");
                    // Continue polling
                }
                SwapStatus::IncompleteDeposit => {
                    tracing::warn!("Incomplete deposit detected");
                    return Ok(SwapStatus::IncompleteDeposit);
                }
            }
        }

        tracing::warn!("Swap status polling timed out");
        Err(AppError::ValidationError(
            "Swap did not complete within timeout".to_string(),
        ))
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
    ) -> AppResult<FinalExecutionStatus> {
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

        if status == SwapStatus::Success {
            Ok(FinalExecutionStatus::SuccessValue("".as_bytes().to_vec()))
        } else {
            tracing::error!(status = ?status, "Swap did not succeed");
            Err(AppError::ValidationError(format!(
                "Swap failed with status: {status:?}"
            )))
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
        // 1-Click API only supports NEP-245 (NEAR Intents) tokens
        // These are cross-chain assets wrapped in the intents.near contract
        from_asset.clone().into_nep245().is_some() && to_asset.clone().into_nep245().is_some()
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
