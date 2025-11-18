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
    let chains = vec![ChainStatus {
        name: "near".to_string(),
        available: true,
        priority: 0,
    }];

    // Update metrics for chain availability
    crate::metrics::set_chain_availability("near", true);

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

    #[tokio::test]
    async fn test_health_endpoint_healthy() {
        let app = create_test_app();
        let response = health(State(app)).await.into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
