//! Status endpoint - Query operation status via Bridge API
//!
//! Queries the NEAR Intents Bridge API to get the status of deposits and withdrawals.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use tracing::{error, info};

use crate::app::App;

use super::models::{DepositStatusResponse, WithdrawalStatusResponse};

/// Query parameters for status endpoint
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    /// Type of operation: "deposit" or "withdrawal"
    #[serde(rename = "type")]
    pub op_type: Option<String>,
}

/// GET /status/withdrawal/:tx_hash - Get withdrawal status from Bridge API
///
/// Queries the Bridge API for the status of a withdrawal by NEAR transaction hash.
#[tracing::instrument(name = "get_withdrawal_status", skip(app), fields(tx_hash = %tx_hash))]
pub async fn get_withdrawal_status(
    State(app): State<App>,
    Path(tx_hash): Path<String>,
) -> impl IntoResponse {
    info!("Querying withdrawal status from Bridge API");

    match app.bridge_client.get_withdrawal_status(&tx_hash).await {
        Ok(status) => {
            info!(
                status = %status.status,
                "Got withdrawal status"
            );
            (
                StatusCode::OK,
                Json(WithdrawalStatusResponse {
                    near_tx_hash: tx_hash,
                    status: status.status.to_string(),
                    chain: status.chain,
                    destination_tx_hash: status.destination_tx_hash,
                    amount: Some(status.amount),
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to get withdrawal status");
            let response = serde_json::json!({
                "error": "BRIDGE_API_ERROR",
                "message": format!("Failed to query Bridge API: {}", e)
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response()
        }
    }
}

/// GET /status/deposit/:tx_hash - Get deposit status from Bridge API
///
/// Queries the Bridge API for the status of a deposit by source chain transaction hash.
#[tracing::instrument(name = "get_deposit_status", skip(app), fields(tx_hash = %tx_hash))]
pub async fn get_deposit_status(
    State(app): State<App>,
    Path(tx_hash): Path<String>,
    Query(query): Query<StatusQuery>,
) -> impl IntoResponse {
    info!("Querying deposit status from Bridge API");

    // Determine chain from query or use default
    let chain = query.op_type.unwrap_or_else(|| "eth".to_string());

    match app.bridge_client.get_deposit_status(&tx_hash, &chain).await {
        Ok(status) => {
            info!(
                status = %status.status,
                "Got deposit status"
            );
            (
                StatusCode::OK,
                Json(DepositStatusResponse {
                    tx_hash,
                    status: status.status,
                    chain: status.chain.unwrap_or_default(),
                    near_tx_hash: status.near_tx_hash,
                    amount: status.amount,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to get deposit status");
            let response = serde_json::json!({
                "error": "BRIDGE_API_ERROR",
                "message": format!("Failed to query Bridge API: {}", e)
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response()
        }
    }
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
            eth_withdraw_address: None,
            arbitrum_withdraw_address: None,
            base_withdraw_address: None,
            optimism_withdraw_address: None,
            polygon_withdraw_address: None,
            solana_withdraw_address: None,
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

    #[tokio::test]
    async fn test_get_withdrawal_status_api_error() {
        let app = create_test_app();

        // This will fail because we're using a test API URL
        let response = get_withdrawal_status(State(app), Path("test-tx-hash".to_string()))
            .await
            .into_response();

        // Should return error since test API is not real
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_get_deposit_status_api_error() {
        let app = create_test_app();

        let query = StatusQuery {
            op_type: Some("eth".to_string()),
        };

        // This will fail because we're using a test API URL
        let response = get_deposit_status(State(app), Path("0xtest".to_string()), Query(query))
            .await
            .into_response();

        // Should return error since test API is not real
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
