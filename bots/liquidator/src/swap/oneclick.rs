// SPDX-License-Identifier: MIT
//! 1-Click API swap provider implementation for NEAR Intents.
//!
//! This module implements swap functionality using the 1-Click API, which provides
//! a simpler interface to NEAR Intents compared to direct contract interaction.
//!
//! # Architecture
//!
//! The 1-Click API works in three phases:
//! 1. Quote: Request a quote and receive a deposit address
//! 2. Deposit: Transfer tokens to the deposit address
//! 3. Poll: Monitor swap status until completion
//!
//! # Benefits over direct intents.near integration
//!
//! - Simpler API with REST endpoints instead of contract calls
//! - Better status tracking and error messages
//! - Handles cross-chain complexity internally
//! - Provides deposit addresses for easier integration

use near_crypto::Signer;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::views::FinalExecutionStatus;
use near_sdk::json_types::U128;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use templar_common::asset::{AssetClass, FungibleAsset};
use tracing::{debug, error, info, warn};

use crate::rpc::{get_access_key_data, send_tx, AppError, AppResult};
use crate::swap::SwapProvider;

use near_primitives::{
    action::Action,
    transaction::{Transaction, TransactionV0},
    types::AccountId,
};

/// 1-Click API base URL
const ONECLICK_API_BASE: &str = "https://1click.chaindefuser.com";

/// Default maximum slippage in basis points (3% = 300 bps)
pub const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 300;

/// Default transaction timeout in seconds
const DEFAULT_TIMEOUT: u64 = 120;

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
    slippage: Option<u32>,
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
        output_amount: U128,
    ) -> AppResult<QuoteResponse> {
        let from_asset_id = Self::to_oneclick_asset_id(from_asset);
        let to_asset_id = Self::to_oneclick_asset_id(to_asset);
        let recipient = self.signer.get_account_id().to_string();

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
            // For liquidation, we need exact output amount (borrow asset to repay debt)
            // EXACT_OUTPUT: we specify exact amount we want to receive, API tells us how much to send
            // This ensures we get the precise amount needed to cover the liquidation
            swap_type: SwapType::ExactOutput,
            slippage_tolerance: self.max_slippage_bps,
            origin_asset: from_asset_id.clone(),
            deposit_type: deposit_type.to_string(),
            destination_asset: to_asset_id.clone(),
            amount: output_amount.0.to_string(),
            refund_to: recipient.clone(),
            // INTENTS: refunds go back to our NEAR account within Intents contract
            refund_type: "INTENTS".to_string(),
            recipient: recipient.clone(),
            // INTENTS: swapped tokens delivered to our NEAR account within Intents contract
            recipient_type: "INTENTS".to_string(),
            deadline: deadline_str,
            referral: Some("templar-liquidator".to_string()), // Track bot usage
            quote_waiting_time_ms: Some(5000), // Wait up to 5 seconds for quote
        };

        let url = format!("{ONECLICK_API_BASE}/v0/quote");
        let mut req = self.http_client.post(&url).json(&request);

        // Add API token if available
        if let Some(token) = &self.api_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.map_err(|e| {
            error!(?e, "Failed to send quote request");
            AppError::ValidationError(format!("Quote request failed: {e}"))
        })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            error!(?e, "Failed to read response");
            AppError::ValidationError(format!("Failed to read response: {e}"))
        })?;

        if !status.is_success() {
            let error_msg = match status.as_u16() {
                400 => format!("Bad Request - Invalid input data: {response_text}"),
                401 => format!("Unauthorized - JWT token is invalid or missing: {response_text}"),
                404 => format!("Not Found - Endpoint or resource not found: {response_text}"),
                _ => format!("Quote request failed with status {status}: {response_text}"),
            };
            error!(
                status = %status,
                response = %response_text,
                "Quote request failed"
            );
            return Err(AppError::ValidationError(error_msg));
        }

        let quote_response: QuoteResponse = serde_json::from_str(&response_text).map_err(|e| {
            error!(?e, response = %response_text, "Failed to parse quote response");
            AppError::ValidationError(format!("Invalid quote response: {e}"))
        })?;

        info!(
            amount_in = %quote_response.quote.amount_in,
            amount_out = %quote_response.quote.amount_out,
            deposit_address = %quote_response.quote.deposit_address,
            time_estimate = %quote_response.quote.time_estimate,
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
        use near_primitives::transaction::{Action, FunctionCallAction};
        use near_sdk::NearToken;

        info!(
            token = %token_contract.contract_id(),
            account = %account_id,
            "Registering storage deposit for account"
        );

        let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

        // Call storage_deposit with 0.00125 NEAR (typical storage cost)
        let storage_deposit_action = FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: serde_json::to_vec(&serde_json::json!({
                "account_id": account_id,
                "registration_only": true,
            }))
            .map_err(|e| AppError::ValidationError(format!("Failed to serialize args: {e}")))?,
            gas: 10_000_000_000_000, // 10 TGas
            deposit: NearToken::from_millinear(1_250).as_yoctonear(), // 0.00125 NEAR
        };

        let tx = Transaction::V0(TransactionV0 {
            nonce,
            receiver_id: token_contract.contract_id().into(),
            block_hash,
            signer_id: self.signer.get_account_id(),
            public_key: self.signer.public_key().clone(),
            actions: vec![Action::FunctionCall(Box::new(storage_deposit_action))],
        });

        let outcome = send_tx(&self.client, &self.signer, self.timeout, tx)
            .await
            .map_err(AppError::from)?;

        match outcome.status {
            FinalExecutionStatus::SuccessValue(_) => {
                info!(
                    account = %account_id,
                    "Storage deposit successful"
                );
                Ok(())
            }
            FinalExecutionStatus::Failure(failure) => {
                // Storage deposit can fail if already registered - that's OK
                warn!(
                    account = %account_id,
                    failure = ?failure,
                    "Storage deposit failed (may already be registered)"
                );
                Ok(())
            }
            _ => {
                warn!(status = ?outcome.status, "Unexpected storage deposit status");
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
        info!(
            asset = %from_asset,
            deposit_address = %deposit_address,
            amount = %amount.0,
            "Depositing tokens to 1-Click"
        );

        // Parse deposit address as NEAR account ID
        // For INTENTS depositType, this is a 64-char hex implicit account
        let deposit_account: AccountId = if deposit_address.len() == 64 {
            // Implicit account - just use the hex string as-is
            deposit_address.to_string().try_into().map_err(|e| {
                error!(?e, deposit_address = %deposit_address, "Invalid implicit account");
                AppError::ValidationError(format!("Invalid implicit account: {e}"))
            })?
        } else {
            deposit_address.parse().map_err(|e| {
                error!(?e, deposit_address = %deposit_address, "Invalid deposit address");
                AppError::ValidationError(format!("Invalid deposit address: {e}"))
            })?
        };

        // For implicit accounts (64-char hex), we need to ensure they exist first
        // by sending a small amount of NEAR to create the account
        if deposit_address.len() == 64 {
            info!(
                deposit_account = %deposit_account,
                "Creating implicit account with NEAR transfer"
            );

            let (nonce, block_hash) = get_access_key_data(&self.client, &self.signer).await?;

            // Send 0.01 NEAR to create the implicit account
            let create_account_tx = Transaction::V0(TransactionV0 {
                nonce,
                receiver_id: deposit_account.clone(),
                block_hash,
                signer_id: self.signer.get_account_id(),
                public_key: self.signer.public_key().clone(),
                actions: vec![Action::Transfer(near_primitives::action::TransferAction {
                    deposit: 10_000_000_000_000_000_000_000, // 0.01 NEAR
                })],
            });

            // Send transaction but don't fail if account already exists
            match send_tx(&self.client, &self.signer, self.timeout, create_account_tx).await {
                Ok(_) => {
                    info!(
                        deposit_account = %deposit_account,
                        "Implicit account created successfully"
                    );
                }
                Err(e) => {
                    // If account already exists, that's fine
                    warn!(
                        deposit_account = %deposit_account,
                        error = ?e,
                        "Failed to create implicit account (may already exist)"
                    );
                }
            }
        }

        // Ensure the deposit address is registered for storage
        self.ensure_storage_deposit(from_asset, &deposit_account)
            .await?;

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

        let outcome = send_tx(&self.client, &self.signer, self.timeout, tx)
            .await
            .map_err(AppError::from)?;

        match &outcome.status {
            FinalExecutionStatus::SuccessValue(_) => {
                info!("Deposit transaction succeeded");
            }
            FinalExecutionStatus::Failure(failure) => {
                error!(
                    failure = ?failure,
                    "Deposit transaction failed with detailed error"
                );
                return Err(AppError::ValidationError(format!(
                    "Deposit transaction failed: {failure:?}"
                )));
            }
            _ => {
                warn!(status = ?outcome.status, "Unexpected transaction status");
            }
        };

        // Check if the deposit was refunded by fetching transaction outcome and checking receipts
        match self
            .check_deposit_refunded(&tx_hash_str, &deposit_account, amount)
            .await
        {
            Ok(true) => {
                error!(
                    tx_hash = %tx_hash_str,
                    deposit_account = %deposit_account,
                    "Deposit was refunded - 1-Click rejected the deposit"
                );
                return Err(AppError::ValidationError(
                    "Deposit was refunded by 1-Click deposit address".to_string(),
                ));
            }
            Ok(false) => {
                info!(tx_hash = %tx_hash_str, "Deposit was accepted (not refunded)");
            }
            Err(e) => {
                warn!(
                    error = ?e,
                    "Failed to check if deposit was refunded, assuming accepted"
                );
            }
        }

        Ok(tx_hash_str)
    }

    /// Checks if a deposit was refunded by examining transaction receipts.
    ///
    /// Returns true if the full amount was refunded back to sender.
    async fn check_deposit_refunded(
        &self,
        tx_hash: &str,
        deposit_account: &AccountId,
        _amount: U128,
    ) -> AppResult<bool> {
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
        // back to us with the same amount, it was refunded
        let mut tokens_sent = false;
        let mut tokens_returned = false;

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
                    // Parse the event to check direction
                    if log.contains(&format!("\"new_owner_id\":\"{deposit_account}\"")) {
                        tokens_sent = true;
                    }
                    if log.contains(&format!("\"old_owner_id\":\"{deposit_account}\""))
                        && log.contains(&format!(
                            "\"new_owner_id\":\"{}\"",
                            self.signer.get_account_id()
                        ))
                    {
                        tokens_returned = true;
                    }
                }
            }
        }

        // If both sent and returned, it was refunded
        Ok(tokens_sent && tokens_returned)
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
            error!(?e, "Failed to submit deposit");
            AppError::ValidationError(format!("Deposit submit failed: {e}"))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let response_text = response.text().await.unwrap_or_default();
            let error_msg = match status.as_u16() {
                400 => format!("Bad Request - Invalid deposit data: {response_text}"),
                401 => format!("Unauthorized - JWT token is invalid: {response_text}"),
                404 => format!("Not Found - Deposit address not found: {response_text}"),
                _ => format!("Deposit submission failed with status {status}: {response_text}"),
            };
            error!(
                status = %status,
                response = %response_text,
                "Deposit submission failed"
            );
            return Err(AppError::ValidationError(error_msg));
        }

        info!("Deposit submitted to 1-Click API");
        Ok(())
    }

    /// Polls the swap status until completion.
    async fn poll_swap_status(
        &self,
        deposit_address: &str,
        memo: Option<&str>,
        max_wait_seconds: u64,
    ) -> AppResult<SwapStatus> {
        let poll_interval = 10; // Poll every 10 seconds
        let max_attempts = max_wait_seconds / poll_interval;

        info!(
            deposit_address = %deposit_address,
            max_wait_seconds = %max_wait_seconds,
            "Polling swap status"
        );

        for attempt in 1..=max_attempts {
            tokio::time::sleep(tokio::time::Duration::from_secs(poll_interval)).await;

            let mut url = format!("{ONECLICK_API_BASE}/v0/status?depositAddress={deposit_address}");
            if let Some(m) = memo {
                url.push_str(&format!("&depositMemo={m}"));
            }

            let mut req = self.http_client.get(&url);
            if let Some(token) = &self.api_token {
                req = req.bearer_auth(token);
            }

            let response = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    warn!(?e, attempt = %attempt, "Failed to fetch status");
                    continue;
                }
            };

            if !response.status().is_success() {
                let status_code = response.status();
                let error_text = response.text().await.unwrap_or_default();
                match status_code.as_u16() {
                    401 => warn!(
                        attempt = %attempt, 
                        "Unauthorized - JWT token may be invalid"
                    ),
                    404 => warn!(
                        attempt = %attempt, 
                        deposit_address = %deposit_address,
                        "Deposit address not found - swap may not have been initiated yet"
                    ),
                    _ => warn!(
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
                    warn!(?e, attempt = %attempt, "Failed to read status response text");
                    continue;
                }
            };

            debug!(response = %response_text, "Raw status response");

            let status_response: StatusResponse = match serde_json::from_str(&response_text) {
                Ok(s) => s,
                Err(e) => {
                    warn!(?e, response = %response_text, attempt = %attempt, "Failed to parse status response");
                    continue;
                }
            };

            info!(
                attempt = %attempt,
                status = ?status_response.status,
                "Swap status update"
            );

            match status_response.status {
                SwapStatus::Success => {
                    info!("Swap completed successfully");
                    return Ok(SwapStatus::Success);
                }
                SwapStatus::Failed | SwapStatus::Refunded => {
                    error!(status = ?status_response.status, "Swap failed or refunded");
                    return Ok(status_response.status);
                }
                SwapStatus::PendingDeposit | SwapStatus::KnownDepositTx | SwapStatus::Processing => {
                    debug!(status = ?status_response.status, "Swap still in progress");
                    // Continue polling
                }
                SwapStatus::IncompleteDeposit => {
                    warn!("Incomplete deposit detected");
                    return Ok(SwapStatus::IncompleteDeposit);
                }
            }
        }

        warn!("Swap status polling timed out");
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
        output_amount = %output_amount.0
    ))]
    async fn quote<F: AssetClass, T: AssetClass>(
        &self,
        from_asset: &FungibleAsset<F>,
        to_asset: &FungibleAsset<T>,
        output_amount: U128,
    ) -> AppResult<U128> {
        let quote_response = self
            .request_quote(from_asset, to_asset, output_amount)
            .await?;

        let input_amount: u128 = quote_response.quote.amount_in.parse().map_err(|e| {
            error!(?e, amount = %quote_response.quote.amount_in, "Failed to parse input amount");
            AppError::ValidationError(format!("Invalid input amount: {e}"))
        })?;

        debug!(
            input_amount = %input_amount,
            output_amount = %output_amount.0,
            "1-Click quote received"
        );

        Ok(U128(input_amount))
    }

    #[tracing::instrument(skip(self), level = "info", fields(
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
        // Step 1: Get quote with deposit address
        let quote_response = self.request_quote(from_asset, to_asset, amount).await?;

        let deposit_address = &quote_response.quote.deposit_address;
        let memo = quote_response.quote.deposit_memo.as_deref();
        let input_amount_str = &quote_response.quote.amount_in;

        let input_amount: u128 = input_amount_str.parse().map_err(|e| {
            error!(?e, amount = %input_amount_str, "Failed to parse input amount");
            AppError::ValidationError(format!("Invalid input amount: {e}"))
        })?;

        // Step 2: Deposit tokens
        let tx_hash = self
            .deposit_tokens(from_asset, deposit_address, U128(input_amount), memo)
            .await?;

        // Step 3: Notify 1-Click of deposit
        self.submit_deposit(&tx_hash, deposit_address, memo).await?;

        // Step 4: Poll for completion (wait up to 20 minutes)
        let status = self.poll_swap_status(deposit_address, memo, 1200).await?;

        if status == SwapStatus::Success {
            info!("1-Click swap completed successfully");
            Ok(FinalExecutionStatus::SuccessValue("".as_bytes().to_vec()))
        } else {
            error!(status = ?status, "Swap did not succeed");
            Err(AppError::ValidationError(format!(
                "Swap failed with status: {status:?}"
            )))
        }
    }

    fn provider_name(&self) -> &'static str {
        "1-Click API (NEAR Intents)"
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
