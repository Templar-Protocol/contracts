//! Error taxonomy for the liquidator.
//!
//! The blockchain plumbing that used to live here (view/send_tx/nonce tracking)
//! has been replaced by the in-process gateway client
//! ([`templar_gateway_client`]). What remains is the error taxonomy that the rest
//! of the crate — and its notification-classification tests — depend on:
//!
//! - [`RpcError`] — low-level error kinds, including the timeout/wrong-response
//!   variants the notifier classifies on.
//! - [`AppError`] — application-level wrapper used by the swap providers.
//! - [`ContractSourceMetadata`] / [`Standard`] — NEP-330 metadata shapes.
//!
//! Gateway errors are mapped into [`RpcError`] via [`From`] so the existing
//! classification keeps working unchanged.

use near_jsonrpc_client::{
    errors::{JsonRpcError, JsonRpcServerError},
    methods::{query::RpcQueryError, tx::RpcTransactionError},
};
use near_sdk::serde::{Deserialize, Serialize};
use templar_gateway_core::GatewayError;

/// Error types for RPC operations
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// Failed to query view method
    #[error("Failed to query view method: {0}")]
    ViewMethodError(#[from] JsonRpcError<RpcQueryError>),
    /// Failed to get access key data
    #[error("Failed to get access key data: {0}")]
    AccessKeyDataError(JsonRpcError<RpcQueryError>),
    /// Got wrong response kind from RPC
    #[error("Got wrong response kind from RPC: {0}")]
    WrongResponseKind(String),
    /// Failed to send transaction
    #[error("Failed to send transaction: {0}")]
    SendTransactionError(#[from] JsonRpcError<RpcTransactionError>),
    /// Failed to deserialize response
    #[error("Failed to deserialize response: {0}")]
    DeserializeError(#[from] near_sdk::serde_json::Error),
    /// Timeout exceeded
    #[error("Timeout exceeded after {0}s (waited {1}s)")]
    TimeoutError(u64, u64),
    /// No outcome for transaction
    #[error("No outcome for transaction: {0}")]
    NoOutcome(String),
}

impl RpcError {
    pub fn is_method_not_found(&self) -> bool {
        fn has_error_token(vm_error: &str, token: &str) -> bool {
            vm_error
                .trim()
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|part| part == token)
        }

        // NEAR currently exposes this as an unstructured VM error string; switch
        // to a structured field if the RPC API starts returning one.
        matches!(
            self,
            Self::ViewMethodError(JsonRpcError::ServerError(JsonRpcServerError::HandlerError(
                RpcQueryError::ContractExecutionError { vm_error, .. }
            ))) if has_error_token(vm_error, "methodnotfound")
                || has_error_token(vm_error, "methodresolveerror")
        )
    }
}

/// Returns `true` if a [`GatewayError`] describes a missing contract method.
///
/// Gateway reads surface contract execution failures as unstructured strings, so
/// this inspects the rendered error message for the NEAR `MethodNotFound` /
/// `MethodResolveError` tokens — the same detection the structured
/// [`RpcError::is_method_not_found`] performs.
pub fn gateway_is_method_not_found(error: &GatewayError) -> bool {
    fn has_error_token(vm_error: &str, token: &str) -> bool {
        vm_error
            .trim()
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == token)
    }

    let message = error.to_string();
    has_error_token(&message, "methodnotfound") || has_error_token(&message, "methodresolveerror")
}

impl From<GatewayError> for RpcError {
    fn from(error: GatewayError) -> Self {
        let message = error.to_string();
        let lowered = message.to_lowercase();
        if lowered.contains("timeout") || lowered.contains("timed out") {
            // The gateway does not surface the configured/elapsed seconds, so
            // report zero for both — the notifier only classifies on the variant.
            RpcError::TimeoutError(0, 0)
        } else {
            RpcError::WrongResponseKind(message)
        }
    }
}

/// Error types for application-level operations
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// RPC operation failed
    #[error("RPC error: {0}")]
    Rpc(#[from] RpcError),
    /// Validation error
    #[error("Validation error: {0}")]
    ValidationError(String),
    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

pub type RpcResult<T = ()> = Result<T, RpcError>;
pub type AppResult<T = ()> = Result<T, AppError>;

/// Contract source metadata as defined by NEP-330
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContractSourceMetadata {
    /// Contract version (semver format)
    pub version: String,
    /// Link to source code repository
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Standards implemented by the contract
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standards: Option<Vec<Standard>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Standard {
    pub standard: String,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_error_display() {
        let error = RpcError::WrongResponseKind("unexpected type".to_string());
        let display = format!("{error}");
        assert!(display.contains("unexpected type"));
    }

    #[test]
    fn test_app_error_from_rpc_error() {
        let rpc_error = RpcError::WrongResponseKind("test".to_string());
        let app_error: AppError = rpc_error.into();
        let display = format!("{app_error}");
        assert!(display.contains("RPC error"));
    }

    #[test]
    fn test_timeout_error_display() {
        let error = RpcError::TimeoutError(60, 65);
        let display = format!("{error}");
        assert!(display.contains("60"));
        assert!(display.contains("65"));
    }

    #[test]
    fn test_gateway_error_maps_to_timeout() {
        let error = GatewayError::NearTransaction("request timed out after 30s".to_string());
        assert!(matches!(
            RpcError::from(error),
            RpcError::TimeoutError(_, _)
        ));
    }

    #[test]
    fn test_gateway_error_maps_to_wrong_response_kind() {
        let error = GatewayError::NearQuery("some other failure".to_string());
        assert!(matches!(
            RpcError::from(error),
            RpcError::WrongResponseKind(_)
        ));
    }

    #[test]
    fn test_gateway_method_not_found_detection() {
        let error = GatewayError::NearQuery(
            "ContractExecutionError: MethodResolveError(MethodNotFound)".to_string(),
        );
        assert!(gateway_is_method_not_found(&error));

        let other = GatewayError::NearQuery("some other failure".to_string());
        assert!(!gateway_is_method_not_found(&other));
    }
}
