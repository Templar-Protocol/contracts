use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing, Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{
    bridge_transport::{BridgeRelayer, DepositCompletion, HotBridgeRelayer},
    config::{AuthToken, ValidatedConfig},
    hot_relayer::{
        HotMpcApiClient, HotRelayerError, PendingWithdrawal, StellarDepositEvent,
        StellarWithdrawExecution,
    },
    metrics, Config, VERSION,
};

#[derive(Clone)]
pub struct AppState {
    relayer: Arc<dyn BridgeRelayer + Send + Sync>,
    auth_token: AuthToken,
}

impl AppState {
    pub fn new(config: &Config) -> Result<Self, HotRelayerError> {
        let validated = config
            .validate()
            .map_err(|error| HotRelayerError::InvalidRouting {
                field: "config",
                reason: error.to_string(),
            })?;
        Self::from_validated_config(validated)
    }

    pub fn from_validated_config(config: ValidatedConfig) -> Result<Self, HotRelayerError> {
        let signer = HotMpcApiClient::new(
            config.hot_mpc_api_url().clone(),
            config.mpc_timeout().duration(),
        )?;
        Ok(Self {
            relayer: Arc::new(HotBridgeRelayer::new(config.routing().clone(), signer)),
            auth_token: config.auth_token().clone(),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompleteDepositRequest {
    pub event: StellarDepositEvent,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompleteDepositResponse {
    pub signature: String,
    pub signed_receiver: String,
    pub signed_nonce: String,
}

impl From<DepositCompletion> for CompleteDepositResponse {
    fn from(value: DepositCompletion) -> Self {
        Self {
            signature: value.signature,
            signed_receiver: value.sign_request.receiver_id,
            signed_nonce: value.sign_request.nonce,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompleteWithdrawalRequest {
    pub pending: PendingWithdrawal,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompleteWithdrawalResponse {
    pub execution: StellarWithdrawExecution,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", routing::get(health))
        .route("/metrics", routing::get(metrics::metrics))
        .route("/relay/deposit/complete", routing::post(complete_deposit))
        .route(
            "/relay/withdrawal/complete",
            routing::post(complete_withdrawal),
        )
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "healthy": true,
        "service": "hot-relayer",
        "version": VERSION,
    }))
}

#[tracing::instrument(name = "hot_relay_complete_deposit", skip(state, headers, req))]
async fn complete_deposit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteDepositRequest>,
) -> Response {
    if let Some(response) = require_auth(&state, &headers) {
        return response;
    }

    match state.relayer.complete_deposit(&req.event).await {
        Ok(result) => (StatusCode::OK, Json(CompleteDepositResponse::from(result))).into_response(),
        Err(error) => map_relayer_error(error),
    }
}

#[tracing::instrument(name = "hot_relay_complete_withdrawal", skip(state, headers, req))]
async fn complete_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteWithdrawalRequest>,
) -> Response {
    if let Some(response) = require_auth(&state, &headers) {
        return response;
    }

    match state.relayer.complete_withdrawal(&req.pending).await {
        Ok(execution) => (
            StatusCode::OK,
            Json(CompleteWithdrawalResponse { execution }),
        )
            .into_response(),
        Err(error) => map_relayer_error(error),
    }
}

fn require_auth(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let expected_header = state.auth_token.bearer_header();
    let is_authorized = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| actual == expected_header);

    if is_authorized {
        None
    } else {
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Unauthorized relay request"
                })),
            )
                .into_response(),
        )
    }
}

fn map_relayer_error(error: HotRelayerError) -> Response {
    let status = match &error {
        HotRelayerError::UnexpectedReceiver { .. }
        | HotRelayerError::InvalidField { .. }
        | HotRelayerError::InvalidRouting { .. } => StatusCode::BAD_REQUEST,
        HotRelayerError::Http(_)
        | HotRelayerError::HttpStatus { .. }
        | HotRelayerError::Decode(_) => StatusCode::BAD_GATEWAY,
    };

    (
        status,
        Json(serde_json::json!({
            "error": error.to_string(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::{config::HotMpcApiUrl, hot_relayer::HotRelayerRouting};

    fn state() -> AppState {
        let routing = HotRelayerRouting::new(
            "vault-counterparty.near".to_string(),
            "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV".to_string(),
            1100,
            "1100_CUSDC".to_string(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let signer = HotMpcApiClient::new(
            HotMpcApiUrl::parse("http://127.0.0.1:9").unwrap(),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        AppState {
            relayer: Arc::new(HotBridgeRelayer::new(routing, signer)),
            auth_token: AuthToken::new("relay-secret").unwrap(),
        }
    }

    #[tokio::test]
    async fn health_route_is_public() {
        let app = router(state());
        let response = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["service"], "hot-relayer");
    }

    #[tokio::test]
    async fn metrics_route_is_public() {
        let app = router(state());
        let response = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/plain"));
    }

    #[tokio::test]
    async fn funding_bridge_routes_are_not_exposed() {
        let app = router(state());
        let response = app
            .oneshot(Request::post("/withdraw").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn relay_routes_require_auth() {
        let app = router(state());
        let response = app
            .oneshot(
                Request::post("/relay/deposit/complete")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"event":{"chain_id":1100,"nonce":"1","sender_id":"GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV","receiver_id":"vault-counterparty.near","token_id":"1100_CUSDC","amount":"1"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn withdrawal_relay_route_requires_auth() {
        let app = router(state());
        let response = app
            .oneshot(
                Request::post("/relay/withdrawal/complete")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"pending":{"nonce":"1","chain_id":1100,"withdraw_data":{"receiver_id":"GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV","token_id":"1100_CUSDC","amount":"1"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorized_invalid_payload_is_bad_request() {
        let app = router(state());
        let response = app
            .oneshot(
                Request::post("/relay/deposit/complete")
                    .header(AUTHORIZATION, "Bearer relay-secret")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"event":{"chain_id":1101,"nonce":"1","sender_id":"GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV","receiver_id":"vault-counterparty.near","token_id":"1100_CUSDC","amount":"1"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
