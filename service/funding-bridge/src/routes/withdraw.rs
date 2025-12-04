//! Withdraw endpoint - Withdraw funds from NEAR to external chains

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use tracing::{debug, error, info, warn};

use crate::{app::App, bridge::ChainId};

use super::models::{WithdrawRequest, WithdrawResponse, WithdrawStatus};

/// POST /withdraw - Withdraw funds from NEAR to external chain
///
/// Initiates a withdrawal from NEAR to external chains (EVM, Solana) via the bridge.
/// The destination address is configured per-chain in the service configuration.
/// Returns immediately with status - actual bridge transfer may take time.
#[tracing::instrument(
    name = "withdraw",
    skip(app, req),
    fields(
        dest_chain = %req.destination_chain,
        asset = %req.asset,
        amount = %req.amount
    )
)]
pub async fn withdraw(State(app): State<App>, Json(req): Json<WithdrawRequest>) -> Response {
    // Parse amount
    let amount: u128 = match req.amount.parse() {
        Ok(amt) => amt,
        Err(e) => {
            error!(error = %e, "Invalid amount format");
            return error_response(StatusCode::BAD_REQUEST, format!("Invalid amount: {}", e));
        }
    };

    if amount == 0 {
        error!("Amount must be greater than zero");
        return error_response(
            StatusCode::BAD_REQUEST,
            "Amount must be greater than zero".to_string(),
        );
    }

    // Parse destination chain - support both "ethereum" and "eth:1" formats
    let (chain_id, chain_name) = match parse_chain(&req.destination_chain) {
        Ok(result) => result,
        Err(e) => {
            error!(chain = %req.destination_chain, error = %e, "Invalid destination chain");
            return error_response(StatusCode::BAD_REQUEST, e);
        }
    };

    debug!(
        chain_id = %chain_id,
        chain_name = %chain_name,
        "Parsed destination chain"
    );

    // Get destination address from config
    let destination_address = match app.config.get_withdraw_address(&chain_name) {
        Some(addr) => addr,
        None => {
            error!(chain = %chain_name, "No withdrawal destination configured for chain");
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "No withdrawal destination configured for {}. \
                     Please set {}_WITHDRAW_ADDRESS in configuration.",
                    chain_name,
                    chain_name.to_uppercase()
                ),
            );
        }
    };

    // Validate token is supported by the bridge
    let token_info = match app
        .bridge_client
        .find_token(&req.asset, &chain_id.to_string())
        .await
    {
        Ok(Some(info)) => {
            debug!(
                token = %info.asset_name,
                near_token = %info.near_token_id,
                destination = %destination_address,
                "Token resolved"
            );
            Some(info)
        }
        Ok(None) => {
            warn!(
                asset = %req.asset,
                chain = %chain_id,
                "Token not found in bridge - proceeding anyway"
            );
            None
        }
        Err(e) => {
            warn!(
                error = %e,
                "Failed to query bridge for token info - proceeding anyway"
            );
            None
        }
    };

    // If dry run, return success immediately
    if app.dry_run || req.dry_run {
        info!("Dry run mode - no actual withdrawal");

        // Record metrics
        crate::metrics::record_withdraw("COMPLETED", &chain_name);

        return (
            StatusCode::OK,
            Json(WithdrawResponse {
                source_tx_hash: format!("dry-run-near-tx-{}", amount),
                status: WithdrawStatus::Completed,
                destination_tx_hash: Some(format!("dry-run-{}-tx-{}", chain_name, amount)),
                error: None,
            }),
        )
            .into_response();
    }

    // Prepare withdrawal
    // For NEAR → External chain withdrawals:
    // 1. User must transfer bridged tokens (e.g., eth.omft.near) to the bridge contract
    // 2. Bridge burns/locks tokens and releases on external chain
    // 3. We track the withdrawal status

    let near_token_id = if let Some(info) = &token_info {
        info.near_token_id.clone()
    } else {
        // Fallback: use token registry to resolve OMFT token ID
        match app
            .token_registry
            .resolve_to_omft(&req.asset, &chain_id.to_string())
            .await
        {
            Ok(omft_id) => {
                info!(
                    asset = %req.asset,
                    omft_id = %omft_id,
                    "Resolved asset to OMFT token ID"
                );
                omft_id
            }
            Err(e) => {
                error!(
                    asset = %req.asset,
                    chain = %chain_id,
                    error = %e,
                    "Failed to resolve asset to OMFT token ID"
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Unknown asset '{}' for chain {}: {}",
                        req.asset, chain_id, e
                    ),
                );
            }
        }
    };

    // Build withdrawal intent using NEAR Intents protocol
    let intent_builder = crate::intents::WithdrawalIntentBuilder::new(
        app.near_handler.treasury_account().to_string(),
        app.near_handler.signer_key().clone(),
    );

    // Detect token type and use appropriate withdrawal method
    let is_nep245 = token_info
        .as_ref()
        .map(|info| info.is_nep245())
        .unwrap_or_else(|| near_token_id.starts_with("nep245:") || near_token_id.contains(':'));

    let execute_args = if is_nep245 {
        let token_id = if let Some(info) = &token_info {
            info.withdrawal_token_id()
        } else {
            near_token_id
                .strip_prefix("nep245:")
                .unwrap_or(&near_token_id)
                .to_string()
        };

        debug!(
            token_id = %token_id,
            "Building NEP-245 withdrawal"
        );

        let numeric_chain_id = if let Some(colon_pos) = token_id.find(':') {
            let after_colon = &token_id[colon_pos + 1..];
            if let Some(underscore_pos) = after_colon.find('_') {
                after_colon[..underscore_pos].parse::<u32>().unwrap_or(1100)
            } else {
                1100
            }
        } else if let Some(underscore_pos) = token_id.find('_') {
            token_id[..underscore_pos].parse::<u32>().unwrap_or(1100)
        } else {
            1100
        };

        debug!(
            numeric_chain_id = %numeric_chain_id,
            "Extracted numeric chain ID from token"
        );

        match intent_builder.build_mt_withdrawal(
            &token_id,
            amount,
            &destination_address,
            numeric_chain_id,
        ) {
            Ok(args) => args,
            Err(e) => {
                error!(error = %e, "Failed to build NEP-245 withdrawal intent");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to build withdrawal intent: {}", e),
                );
            }
        }
    } else {
        // NEP-141 fungible token withdrawal
        debug!(
            token = %near_token_id,
            "Building NEP-141 withdrawal"
        );

        match intent_builder.build_withdrawal(&near_token_id, amount, &destination_address) {
            Ok(args) => args,
            Err(e) => {
                error!(error = %e, "Failed to build NEP-141 withdrawal intent");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to build withdrawal intent: {}", e),
                );
            }
        }
    };

    // Execute the withdrawal intent on intents.near
    let tx_hash = match app.near_handler.execute_intents(&execute_args).await {
        Ok(hash) => {
            info!(
                tx_hash = %hash,
                near_token = %near_token_id,
                destination = %destination_address,
                "Withdrawal executed"
            );
            hash
        }
        Err(e) => {
            error!(error = %e, "Failed to execute withdrawal intent");

            // Record metrics
            crate::metrics::record_withdraw("FAILED", &chain_name);

            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to execute withdrawal: {}", e),
            );
        }
    };

    // Record metrics
    crate::metrics::record_withdraw("PENDING", &chain_name);

    // Return pending status with transaction hash
    // Bridge will process the withdrawal asynchronously
    (
        StatusCode::OK,
        Json(WithdrawResponse {
            source_tx_hash: tx_hash,
            status: WithdrawStatus::Pending,
            destination_tx_hash: None,
            error: None,
        }),
    )
        .into_response()
}

/// Parse chain identifier from various formats
/// Supports: "ethereum", "eth:1", "arbitrum", "eth:42161", "solana", "sol:mainnet", etc.
fn parse_chain(chain: &str) -> Result<(ChainId, String), String> {
    // Check if it's already in "type:id" format
    if let Some(parsed) = ChainId::parse(chain) {
        let name = match (parsed.chain_type.as_str(), parsed.chain_id.as_str()) {
            ("eth", "1") => "ethereum",
            ("eth", "42161") => "arbitrum",
            ("eth", "8453") => "base",
            ("eth", "10") => "optimism",
            ("eth", "137") => "polygon",
            ("sol", _) => "solana",
            ("stellar", _) => "stellar",
            _ => "unknown",
        };
        return Ok((parsed, name.to_string()));
    }

    // Parse human-readable names
    match chain.to_lowercase().as_str() {
        "ethereum" | "eth" => Ok((ChainId::ethereum_mainnet(), "ethereum".to_string())),
        "arbitrum" | "arb" => Ok((ChainId::arbitrum(), "arbitrum".to_string())),
        "base" => Ok((ChainId::base(), "base".to_string())),
        "optimism" | "op" => Ok((ChainId::new("eth", "10"), "optimism".to_string())),
        "polygon" | "matic" => Ok((ChainId::new("eth", "137"), "polygon".to_string())),
        "solana" | "sol" => Ok((ChainId::new("sol", "mainnet"), "solana".to_string())),
        "stellar" => Ok((ChainId::new("stellar", "mainnet"), "stellar".to_string())),
        _ => Err(format!(
            "Unsupported destination chain: {}. \
             Supported: ethereum (eth:1), arbitrum (eth:42161), base (eth:8453), \
             optimism (eth:10), polygon (eth:137), solana (sol:mainnet), stellar (stellar:mainnet)",
            chain
        )),
    }
}

fn error_response(status_code: StatusCode, error: String) -> Response {
    (
        status_code,
        Json(WithdrawResponse {
            source_tx_hash: String::new(),
            status: WithdrawStatus::Failed,
            destination_tx_hash: None,
            error: Some(error),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bridge::BridgeClient, chain::NearHandler};
    use near_crypto::{KeyType, SecretKey};
    use near_primitives::types::AccountId;
    use std::{str::FromStr, sync::Arc};

    fn create_test_app() -> App {
        use crate::config::Args;
        use crate::rpc::Network;

        let args = Args {
            port: 3000,
            network: Network::Testnet,
            bridge_api_url: "https://test.api".to_string(),
            dry_run: false,
            near_account: Some(AccountId::from_str("test.near").unwrap()),
            near_signer_key: Some(SecretKey::from_random(KeyType::ED25519)),
            near_rpc_url: None,
            eth_private_key: None,
            eth_rpc_url: "https://eth.llamarpc.com".to_string(),
            solana_private_key: None,
            solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            eth_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
            arbitrum_withdraw_address: Some(
                "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
            ),
            base_withdraw_address: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string()),
            optimism_withdraw_address: None,
            polygon_withdraw_address: None,
            solana_withdraw_address: Some(
                "B4b13ZjqPNGmvK7VVXM3kZ3vEpKS7JVzuqVU6vGqXm9D".to_string(),
            ),
            stellar_secret_key: None,
            stellar_horizon_url: "https://horizon.stellar.org".to_string(),
            stellar_network: "mainnet".to_string(),
            stellar_withdraw_address: None,
        };

        let bridge_client = Arc::new(BridgeClient::new(args.bridge_api_url.clone()));
        let token_registry = crate::tokens::TokenRegistry::new(Arc::clone(&bridge_client));

        let near_handler = Arc::new(NearHandler::new(
            args.near_account.clone().unwrap(),
            args.near_signer_key.clone().unwrap(),
            args.get_near_rpc_url(),
            true,
        ));

        App {
            near_handler,
            bridge_client,
            token_registry,
            external_chains: std::sync::Arc::new(crate::external::ExternalChainRegistry::new()),
            config: Arc::new(args),
            dry_run: false,
            version: "0.1.0-test",
        }
    }

    #[test]
    fn test_parse_chain_ethereum() {
        let (chain_id, name) = parse_chain("ethereum").unwrap();
        assert_eq!(chain_id.to_string(), "eth:1");
        assert_eq!(name, "ethereum");
    }

    #[test]
    fn test_parse_chain_eth_format() {
        let (chain_id, name) = parse_chain("eth:1").unwrap();
        assert_eq!(chain_id.to_string(), "eth:1");
        assert_eq!(name, "ethereum");
    }

    #[test]
    fn test_parse_chain_arbitrum() {
        let (chain_id, name) = parse_chain("arbitrum").unwrap();
        assert_eq!(chain_id.to_string(), "eth:42161");
        assert_eq!(name, "arbitrum");
    }

    #[test]
    fn test_parse_chain_base() {
        let (chain_id, name) = parse_chain("base").unwrap();
        assert_eq!(chain_id.to_string(), "eth:8453");
        assert_eq!(name, "base");
    }

    #[test]
    fn test_parse_chain_invalid() {
        let result = parse_chain("bitcoin");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_withdraw_pending() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "ethereum".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: false,
        };

        let response = withdraw(State(app), Json(req)).await;

        // Should succeed with dry_run NearHandler (returns immediately)
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_withdraw_with_eth_chain_format() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "eth:42161".to_string(), // Arbitrum
            asset: "usdc".to_string(),
            amount: "1000000".to_string(),
            dry_run: false,
        };

        let response = withdraw(State(app), Json(req)).await;

        // Should succeed with dry_run NearHandler
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_withdraw_invalid_amount() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "ethereum".to_string(),
            asset: "usdc".to_string(),
            amount: "invalid".to_string(),
            dry_run: false,
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_zero_amount() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "ethereum".to_string(),
            asset: "usdc".to_string(),
            amount: "0".to_string(),
            dry_run: false,
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_unsupported_chain() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "bitcoin".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: false,
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_dry_run() {
        let app = create_test_app();

        let req = WithdrawRequest {
            destination_chain: "arbitrum".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: true,
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }
}
