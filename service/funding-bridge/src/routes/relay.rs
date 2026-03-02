use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    app::App,
    hot_relayer::{
        HotRelayerError, PendingWithdrawal, StellarDepositEvent, StellarWithdrawExecution,
    },
};

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

#[derive(Debug, Clone, Deserialize)]
pub struct CompleteWithdrawalRequest {
    pub pending: PendingWithdrawal,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompleteWithdrawalResponse {
    pub execution: StellarWithdrawExecution,
}

#[tracing::instrument(name = "relay_complete_deposit", skip(app, req))]
pub async fn complete_deposit(
    State(app): State<App>,
    headers: HeaderMap,
    Json(req): Json<CompleteDepositRequest>,
) -> Response {
    if let Some(response) = require_relay_auth(&app, &headers) {
        return response;
    }

    let Some(relayer) = app.bridge_relayer.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "Bridge relayer backend is not configured"
            })),
        )
            .into_response();
    };

    match relayer.complete_deposit(&req.event).await {
        Ok(result) => (
            StatusCode::OK,
            Json(CompleteDepositResponse {
                signature: result.signature,
                signed_receiver: result.sign_request.receiver_id,
                signed_nonce: result.sign_request.nonce,
            }),
        )
            .into_response(),
        Err(error) => map_relayer_error(error),
    }
}

#[tracing::instrument(name = "relay_complete_withdrawal", skip(app, req))]
pub async fn complete_withdrawal(
    State(app): State<App>,
    headers: HeaderMap,
    Json(req): Json<CompleteWithdrawalRequest>,
) -> Response {
    if let Some(response) = require_relay_auth(&app, &headers) {
        return response;
    }

    let Some(relayer) = app.bridge_relayer.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "Bridge relayer backend is not configured"
            })),
        )
            .into_response();
    };

    match relayer.complete_withdrawal(&req.pending).await {
        Ok(execution) => (
            StatusCode::OK,
            Json(CompleteWithdrawalResponse { execution }),
        )
            .into_response(),
        Err(error) => map_relayer_error(error),
    }
}

fn map_relayer_error(error: HotRelayerError) -> Response {
    let status = match &error {
        HotRelayerError::UnexpectedReceiver { .. } => StatusCode::BAD_REQUEST,
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

fn require_relay_auth(app: &App, headers: &HeaderMap) -> Option<Response> {
    let Some(expected_token) = app.bridge_relayer_auth_token.as_deref() else {
        return None;
    };

    let expected_header = format!("Bearer {expected_token}");
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

#[cfg(test)]
mod tests {
    use axum::{
        extract::State,
        http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    };

    use super::*;
    use crate::routes::test_utils::create_test_app;

    #[tokio::test]
    async fn returns_503_when_backend_not_configured_deposit() {
        let app = create_test_app();
        let req = CompleteDepositRequest {
            event: StellarDepositEvent {
                chain_id: 1100,
                nonce: "1".to_string(),
                sender_id: "G".to_string(),
                receiver_id: "vault-counterparty.near".to_string(),
                token_id: "1100_token".to_string(),
                amount: "1".to_string(),
            },
        };

        let response = complete_deposit(State(app), HeaderMap::new(), Json(req)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_503_when_backend_not_configured_withdrawal() {
        let app = create_test_app();
        let req = CompleteWithdrawalRequest {
            pending: PendingWithdrawal {
                nonce: "1".to_string(),
                chain_id: 1100,
                withdraw_data: crate::hot_relayer::PendingWithdrawData {
                    receiver_id: "GADAPTER".to_string(),
                    amount: "1".to_string(),
                    token_id: "1100_token".to_string(),
                },
            },
        };

        let response = complete_withdrawal(State(app), HeaderMap::new(), Json(req)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_401_when_auth_token_is_configured_and_missing_header() {
        let mut app = create_test_app();
        app.bridge_relayer_auth_token = Some("relay-secret".to_string());
        let req = CompleteDepositRequest {
            event: StellarDepositEvent {
                chain_id: 1100,
                nonce: "1".to_string(),
                sender_id: "G".to_string(),
                receiver_id: "vault-counterparty.near".to_string(),
                token_id: "1100_token".to_string(),
                amount: "1".to_string(),
            },
        };

        let response = complete_deposit(State(app), HeaderMap::new(), Json(req)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn returns_503_when_auth_token_is_configured_and_header_is_valid_but_backend_missing() {
        let mut app = create_test_app();
        app.bridge_relayer_auth_token = Some("relay-secret".to_string());
        let req = CompleteDepositRequest {
            event: StellarDepositEvent {
                chain_id: 1100,
                nonce: "1".to_string(),
                sender_id: "G".to_string(),
                receiver_id: "vault-counterparty.near".to_string(),
                token_id: "1100_token".to_string(),
                amount: "1".to_string(),
            },
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer relay-secret"),
        );

        let response = complete_deposit(State(app), headers, Json(req)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
