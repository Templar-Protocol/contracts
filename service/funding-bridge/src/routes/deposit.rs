//! Deposit endpoint - Automated cross-chain deposits from external wallets
//!
//! Transfers tokens from configured external wallets (ETH/Arbitrum) to the
//! NEAR Intents bridge, which credits the NEAR treasury with OMFT tokens.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use tracing::{error, info};

use crate::app::App;

use super::models::{DepositRequest, DepositResponse};

/// POST /deposit - Execute automated deposit from external wallet
///
/// Transfers tokens from configured external wallet to bridge deposit address.
/// The bridge then credits NEAR treasury with OMFT tokens.
///
/// Requires ETH_PRIVATE_KEY to be configured for Ethereum/Arbitrum deposits.
#[tracing::instrument(
    name = "deposit",
    skip(app),
    fields(
        source_chain = %req.source_chain,
        asset = %req.asset,
        amount = %req.amount
    )
)]
pub async fn deposit(State(app): State<App>, Json(req): Json<DepositRequest>) -> Response {
    info!("Executing automated deposit from external wallet");

    let chain_id = normalize_chain_id(&req.source_chain);

    // Get chain handler from registry
    let chain_handler = match app.external_chains.get(&chain_id) {
        Some(handler) => handler,
        None => {
            error!(chain = %chain_id, "Chain not configured");
            return (
                StatusCode::BAD_REQUEST,
                Json(DepositResponse {
                    source_tx_hash: String::new(),
                    status: "FAILED".to_string(),
                    source_chain: chain_id.clone(),
                    bridge_deposit_address: None,
                    error: Some(format!(
                        "Chain {} not configured. Available chains: {:?}",
                        chain_id,
                        app.available_external_chains()
                    )),
                }),
            )
                .into_response();
        }
    };

    // Check if token is supported
    if !chain_handler.supports_token(&req.asset) {
        error!(asset = %req.asset, chain = %chain_id, "Token not supported");
        return (
            StatusCode::BAD_REQUEST,
            Json(DepositResponse {
                source_tx_hash: String::new(),
                status: "FAILED".to_string(),
                source_chain: chain_id,
                bridge_deposit_address: None,
                error: Some(format!(
                    "Token {} not supported on {}",
                    req.asset,
                    chain_handler.chain_id()
                )),
            }),
        )
            .into_response();
    }

    // Get bridge deposit address for NEAR treasury
    let deposit_address = match app
        .bridge_client
        .get_deposit_address(app.near_handler.treasury_account().as_str(), &chain_id)
        .await
    {
        Ok(result) => {
            info!(
                address = %result.address,
                chain = %result.chain,
                "Got bridge deposit address"
            );
            result.address
        }
        Err(e) => {
            error!(error = %e, "Failed to get deposit address from bridge");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DepositResponse {
                    source_tx_hash: String::new(),
                    status: "FAILED".to_string(),
                    source_chain: chain_id,
                    bridge_deposit_address: None,
                    error: Some(format!("Failed to get bridge deposit address: {}", e)),
                }),
            )
                .into_response();
        }
    };

    // If dry run, return success without executing
    if app.dry_run || req.dry_run {
        info!("Dry run mode - would execute transfer");
        return (
            StatusCode::OK,
            Json(DepositResponse {
                source_tx_hash: "dry-run-tx-hash".to_string(),
                status: "DRY_RUN".to_string(),
                source_chain: chain_id,
                bridge_deposit_address: Some(deposit_address),
                error: None,
            }),
        )
            .into_response();
    }

    // Execute transfer via chain handler
    match chain_handler
        .transfer_tokens(&deposit_address, &req.asset, &req.amount)
        .await
    {
        Ok(result) => {
            info!(
                tx_hash = %result.tx_hash,
                confirmed = %result.confirmed,
                "Transfer executed successfully"
            );
            (
                StatusCode::OK,
                Json(DepositResponse {
                    source_tx_hash: result.tx_hash,
                    status: if result.confirmed {
                        "PENDING".to_string() // Pending bridge processing
                    } else {
                        "SUBMITTED".to_string()
                    },
                    source_chain: chain_id,
                    bridge_deposit_address: Some(deposit_address),
                    error: None,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Transfer failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DepositResponse {
                    source_tx_hash: String::new(),
                    status: "FAILED".to_string(),
                    source_chain: chain_id,
                    bridge_deposit_address: Some(deposit_address),
                    error: Some(format!("Transfer failed: {}", e)),
                }),
            )
                .into_response()
        }
    }
}

/// Normalize chain identifier to standard format
///
/// Converts human-readable names to chain IDs:
/// - "ethereum" -> "eth:1"
/// - "arbitrum" -> "eth:42161"
/// - "solana" -> "sol:mainnet"
/// - "eth:1" -> "eth:1" (unchanged)
pub fn normalize_chain_id(chain: &str) -> String {
    match chain.to_lowercase().as_str() {
        "ethereum" | "eth" => "eth:1".to_string(),
        "arbitrum" | "arb" => "eth:42161".to_string(),
        "base" => "eth:8453".to_string(),
        "optimism" | "op" => "eth:10".to_string(),
        "polygon" | "matic" => "eth:137".to_string(),
        "solana" | "sol" => "sol:mainnet".to_string(),
        _ => chain.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_chain_id_ethereum() {
        assert_eq!(normalize_chain_id("ethereum"), "eth:1");
        assert_eq!(normalize_chain_id("eth"), "eth:1");
        assert_eq!(normalize_chain_id("Ethereum"), "eth:1");
    }

    #[test]
    fn test_normalize_chain_id_arbitrum() {
        assert_eq!(normalize_chain_id("arbitrum"), "eth:42161");
        assert_eq!(normalize_chain_id("arb"), "eth:42161");
    }

    #[test]
    fn test_normalize_chain_id_base() {
        assert_eq!(normalize_chain_id("base"), "eth:8453");
    }

    #[test]
    fn test_normalize_chain_id_solana() {
        assert_eq!(normalize_chain_id("solana"), "sol:mainnet");
        assert_eq!(normalize_chain_id("sol"), "sol:mainnet");
    }

    #[test]
    fn test_normalize_chain_id_passthrough() {
        assert_eq!(normalize_chain_id("eth:1"), "eth:1");
        assert_eq!(normalize_chain_id("eth:42161"), "eth:42161");
        assert_eq!(normalize_chain_id("sol:mainnet"), "sol:mainnet");
    }
}
