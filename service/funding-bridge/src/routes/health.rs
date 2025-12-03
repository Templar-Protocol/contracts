//! Health check endpoint

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::time::Duration;

use crate::app::App;

use super::models::{ChainStatus, HealthResponse, ServiceStatus};

/// GET /health - Service health check
///
/// Returns service health status and external service connectivity
#[tracing::instrument(name = "health_check", skip(app))]
pub async fn health(State(app): State<App>) -> impl IntoResponse {
    let mut chains = vec![ChainStatus {
        name: "near".to_string(),
        available: true,
    }];

    // Update metrics for chain availability
    crate::metrics::set_chain_availability("near", true);

    // Add external chains (for deposits)
    for chain_id in app.available_external_chains() {
        // Parse chain ID to get a readable name
        let chain_name = if chain_id.starts_with("eth:") {
            match chain_id.as_str() {
                "eth:1" => "ethereum",
                "eth:42161" => "arbitrum",
                "eth:8453" => "base",
                "eth:10" => "optimism",
                "eth:137" => "polygon",
                _ => &chain_id,
            }
        } else if chain_id.starts_with("sol:") {
            "solana"
        } else if chain_id.starts_with("stellar:") {
            "stellar"
        } else {
            &chain_id
        };

        chains.push(ChainStatus {
            name: chain_name.to_string(),
            available: true,
        });

        crate::metrics::set_chain_availability(chain_name, true);
    }

    let healthy = app.is_healthy();

    // Check bridge API connectivity (with timeout)
    let bridge_api_status = check_bridge_api_connectivity(&app).await;

    let response = HealthResponse {
        healthy,
        version: app.version.to_string(),
        chains,
        rpc_status: None,
        bridge_api_status: Some(bridge_api_status),
    };

    if healthy {
        (StatusCode::OK, Json(response)).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response()
    }
}

/// Check bridge API connectivity
async fn check_bridge_api_connectivity(app: &App) -> ServiceStatus {
    let start = std::time::Instant::now();

    // Try to get supported tokens with a short timeout
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        app.bridge_client
            .get_supported_tokens(&["eth:1".to_string()]),
    )
    .await;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(_)) => ServiceStatus {
            reachable: true,
            latency_ms: Some(latency_ms),
            error: None,
        },
        Ok(Err(e)) => ServiceStatus {
            reachable: false,
            latency_ms: Some(latency_ms),
            error: Some(e.to_string()),
        },
        Err(_) => ServiceStatus {
            reachable: false,
            latency_ms: Some(latency_ms),
            error: Some("Request timeout".to_string()),
        },
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
    async fn test_health_endpoint_healthy() {
        let app = create_test_app();
        let response = health(State(app)).await.into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
