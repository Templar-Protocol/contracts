//! Withdraw endpoint - Withdraw funds from NEAR to external chains

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use tracing::{debug, error, info, warn};

use crate::{app::App, bridge::ChainId, tracker::OperationInfo};

use super::models::{OperationType, WithdrawRequest, WithdrawResponse, WithdrawStatus};

/// POST /withdraw - Withdraw funds from NEAR to external chain
///
/// Initiates a withdrawal from NEAR to external chains (EVM, Solana) via the bridge.
/// Returns immediately with status - actual bridge transfer may take time.
#[tracing::instrument(
    name = "withdraw",
    skip(app),
    fields(
        request_id = %req.request_id,
        source = %req.source_account,
        dest_chain = %req.destination_chain,
        dest_address = %req.destination_address,
        asset = %req.asset,
        amount = %req.amount
    )
)]
pub async fn withdraw(State(app): State<App>, Json(req): Json<WithdrawRequest>) -> Response {
    info!("Processing withdraw request");

    // Parse amount
    let amount: u128 = match req.amount.parse() {
        Ok(amt) => amt,
        Err(e) => {
            error!(error = %e, "Invalid amount format");
            return error_response(
                &req.request_id,
                StatusCode::BAD_REQUEST,
                format!("Invalid amount: {}", e),
            );
        }
    };

    if amount == 0 {
        error!("Amount must be greater than zero");
        return error_response(
            &req.request_id,
            StatusCode::BAD_REQUEST,
            "Amount must be greater than zero".to_string(),
        );
    }

    // Parse destination chain - support both "ethereum" and "eth:1" formats
    let (chain_id, chain_name) = match parse_chain(&req.destination_chain) {
        Ok(result) => result,
        Err(e) => {
            error!(chain = %req.destination_chain, error = %e, "Invalid destination chain");
            return error_response(&req.request_id, StatusCode::BAD_REQUEST, e);
        }
    };

    debug!(
        chain_id = %chain_id,
        chain_name = %chain_name,
        "Parsed destination chain"
    );

    // Validate token is supported by the bridge
    let token_info = match app
        .bridge_client
        .find_token(&req.asset, &chain_id.to_string())
        .await
    {
        Ok(Some(info)) => {
            info!(
                token = %info.asset_name,
                near_token = %info.near_token_id,
                "Found token in bridge"
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

        // Track dry run operation
        let mut op_info = OperationInfo::new(
            req.request_id.clone(),
            OperationType::Withdraw,
            "COMPLETED".to_string(),
        );
        op_info.add_detail("destination_chain".to_string(), chain_id.to_string());
        op_info.add_detail(
            "destination_address".to_string(),
            req.destination_address.clone(),
        );
        op_info.add_detail("asset".to_string(), req.asset.clone());
        op_info.add_detail("amount".to_string(), amount.to_string());
        op_info.add_detail("dry_run".to_string(), "true".to_string());
        if let Some(info) = &token_info {
            op_info.add_detail("near_token_id".to_string(), info.near_token_id.clone());
        }
        app.tracker.track(op_info);

        // Record metrics
        crate::metrics::record_withdraw("COMPLETED", &chain_name);

        return (
            StatusCode::OK,
            Json(WithdrawResponse {
                request_id: req.request_id,
                status: WithdrawStatus::Completed,
                source_tx_hash: Some(format!("dry-run-near-tx-{}", amount)),
                bridge_tx_id: Some(format!("dry-run-bridge-{}", amount)),
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
                    &req.request_id,
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Unknown asset '{}' for chain {}: {}",
                        req.asset, chain_id, e
                    ),
                );
            }
        }
    };

    info!(
        near_token = %near_token_id,
        destination = %req.destination_address,
        chain = %chain_id,
        amount = %amount,
        "Initiating withdrawal"
    );

    // Build withdrawal intent using NEAR Intents protocol
    let intent_builder = crate::intents::WithdrawalIntentBuilder::new(
        app.near_handler.treasury_account().to_string(),
        app.near_handler.signer_key().clone(),
    );

    let execute_args =
        match intent_builder.build_withdrawal(&near_token_id, amount, &req.destination_address) {
            Ok(args) => args,
            Err(e) => {
                error!(error = %e, "Failed to build withdrawal intent");
                return error_response(
                    &req.request_id,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to build withdrawal intent: {}", e),
                );
            }
        };

    // Execute the withdrawal intent on intents.near
    let tx_hash = match app.near_handler.execute_intents(&execute_args).await {
        Ok(hash) => {
            info!(
                tx_hash = %hash,
                near_token = %near_token_id,
                destination = %req.destination_address,
                amount = %amount,
                "Withdrawal intent executed successfully"
            );
            hash
        }
        Err(e) => {
            error!(error = %e, "Failed to execute withdrawal intent");

            // Track failed operation
            let mut op_info = OperationInfo::new(
                req.request_id.clone(),
                OperationType::Withdraw,
                "FAILED".to_string(),
            );
            op_info.add_detail("destination_chain".to_string(), chain_id.to_string());
            op_info.add_detail(
                "destination_address".to_string(),
                req.destination_address.clone(),
            );
            op_info.add_detail("asset".to_string(), req.asset.clone());
            op_info.add_detail("amount".to_string(), amount.to_string());
            op_info.add_detail("near_token_id".to_string(), near_token_id.clone());
            op_info.add_detail("error".to_string(), e.to_string());
            app.tracker.track(op_info);

            // Record metrics
            crate::metrics::record_withdraw("FAILED", &chain_name);

            return error_response(
                &req.request_id,
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to execute withdrawal: {}", e),
            );
        }
    };

    // Track successful withdrawal
    let mut op_info = OperationInfo::new(
        req.request_id.clone(),
        OperationType::Withdraw,
        "PENDING".to_string(),
    );
    op_info.add_detail("destination_chain".to_string(), chain_id.to_string());
    op_info.add_detail(
        "destination_address".to_string(),
        req.destination_address.clone(),
    );
    op_info.add_detail("asset".to_string(), req.asset.clone());
    op_info.add_detail("amount".to_string(), amount.to_string());
    op_info.add_detail("source_account".to_string(), req.source_account.clone());
    op_info.add_detail("near_token_id".to_string(), near_token_id.clone());
    op_info.add_detail("source_tx_hash".to_string(), tx_hash.clone());
    app.tracker.track(op_info);

    // Record metrics
    crate::metrics::record_withdraw("PENDING", &chain_name);

    info!(
        request_id = %req.request_id,
        tx_hash = %tx_hash,
        near_token = %near_token_id,
        chain = %chain_id,
        "Withdrawal request completed"
    );

    // Return pending status with transaction hash
    // Bridge will process the withdrawal asynchronously
    (
        StatusCode::OK,
        Json(WithdrawResponse {
            request_id: req.request_id,
            status: WithdrawStatus::Pending,
            source_tx_hash: Some(tx_hash),
            bridge_tx_id: None,
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
        _ => Err(format!(
            "Unsupported destination chain: {}. \
             Supported: ethereum (eth:1), arbitrum (eth:42161), base (eth:8453), \
             optimism (eth:10), polygon (eth:137), solana (sol:mainnet)",
            chain
        )),
    }
}

fn error_response(request_id: &str, status_code: StatusCode, error: String) -> Response {
    (
        status_code,
        Json(WithdrawResponse {
            request_id: request_id.to_string(),
            status: WithdrawStatus::Failed,
            source_tx_hash: None,
            bridge_tx_id: None,
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
    use std::{collections::HashMap, str::FromStr, sync::Arc};

    fn create_test_app() -> App {
        let bridge_client = Arc::new(BridgeClient::new("https://test.api".to_string()));
        let token_registry = crate::tokens::TokenRegistry::new(Arc::clone(&bridge_client));

        let near_handler = Arc::new(NearHandler::new(
            AccountId::from_str("test.near").unwrap(),
            SecretKey::from_random(KeyType::ED25519),
            "https://rpc.testnet.near.org".to_string(),
            0,
            true,
        ));

        App {
            near_handler,
            bridge_client,
            token_registry,
            tracker: crate::tracker::OperationTracker::new(),
            external_chains: std::sync::Arc::new(
                crate::external::ExternalChainRegistry::new(),
            ),
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
            request_id: "req-789".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "ethereum".to_string(),
            destination_address: "0x123".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: false,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        // Should succeed with dry_run NearHandler (returns immediately)
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_withdraw_with_eth_chain_format() {
        let app = create_test_app();

        let req = WithdrawRequest {
            request_id: "req-790".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "eth:42161".to_string(), // Arbitrum
            destination_address: "0x456".to_string(),
            asset: "usdc".to_string(),
            amount: "1000000".to_string(),
            dry_run: false,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        // Should succeed with dry_run NearHandler
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_withdraw_invalid_amount() {
        let app = create_test_app();

        let req = WithdrawRequest {
            request_id: "req-789".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "ethereum".to_string(),
            destination_address: "0x123".to_string(),
            asset: "usdc".to_string(),
            amount: "invalid".to_string(),
            dry_run: false,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_zero_amount() {
        let app = create_test_app();

        let req = WithdrawRequest {
            request_id: "req-789".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "ethereum".to_string(),
            destination_address: "0x123".to_string(),
            asset: "usdc".to_string(),
            amount: "0".to_string(),
            dry_run: false,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_unsupported_chain() {
        let app = create_test_app();

        let req = WithdrawRequest {
            request_id: "req-789".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "bitcoin".to_string(),
            destination_address: "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: false,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_withdraw_dry_run() {
        let app = create_test_app();

        let req = WithdrawRequest {
            request_id: "req-789".to_string(),
            source_account: "user.near".to_string(),
            destination_chain: "arbitrum".to_string(),
            destination_address: "0x789".to_string(),
            asset: "usdc".to_string(),
            amount: "500000".to_string(),
            dry_run: true,
            metadata: HashMap::new(),
        };

        let response = withdraw(State(app), Json(req)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }
}
