//! Status endpoint - Query operation status by request ID

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::info;

use crate::app::App;

/// GET /status/:request_id - Get operation status
///
/// Returns the current status of a deposit or withdrawal operation
#[tracing::instrument(name = "get_status", skip(app), fields(request_id = %request_id))]
pub async fn get_status(
    State(app): State<App>,
    Path(request_id): Path<String>,
) -> impl IntoResponse {
    info!("Querying operation status");

    match app.tracker.get(&request_id) {
        Some(info) => {
            let response = info.to_response();
            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let response = serde_json::json!({
                "error": "NOT_FOUND",
                "message": format!("No operation found with request_id: {}", request_id)
            });
            (StatusCode::NOT_FOUND, Json(response)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge::BridgeClient, chain::NearHandler, routes::models::OperationType,
        tracker::OperationInfo,
    };
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
    async fn test_get_status_found() {
        let app = create_test_app();

        // Track an operation
        let info = OperationInfo::new(
            "req-123".to_string(),
            OperationType::Deposit,
            "COMPLETED".to_string(),
        );
        app.tracker.track(info);

        let response = get_status(State(app), Path("req-123".to_string()))
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_status_not_found() {
        let app = create_test_app();

        let response = get_status(State(app), Path("nonexistent".to_string()))
            .await
            .into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
