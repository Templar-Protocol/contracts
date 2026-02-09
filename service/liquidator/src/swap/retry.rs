//! Swap error classification and retry logic.
//!
//! Provides:
//! - `SwapErrorKind` for classifying swap failures as retryable or permanent
//! - `SwapError` wrapper with context
//! - `SwapRetryConfig` for configurable retry behavior
//! - `swap_with_retry` for automatic retry of transient failures

use std::time::Duration;

use tokio::time::sleep;

use crate::rpc::{AppError, AppResult};

/// Classification of swap errors for retry decisions.
#[derive(Debug, Clone)]
pub enum SwapErrorKind {
    /// Amount below bridge/swap minimum (not retryable, batchable)
    AmountTooLow { message: String },

    /// Generic quote failure — may be transient (retryable)
    QuoteFailed { message: String },

    /// Network/connection error (retryable)
    NetworkError { message: String },

    /// Server error 5xx (retryable)
    ServerError { status: u16, message: String },

    /// Rate limited 429 (retryable)
    RateLimited,

    /// Client validation error 400 (not retryable)
    ValidationError { message: String },

    /// Swap timed out waiting for completion (retryable)
    Timeout { message: String },

    /// Unknown / uncategorized error (not retryable)
    Unknown { message: String },
}

impl SwapErrorKind {
    /// Returns true if this error type should be retried.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::QuoteFailed { .. }
                | Self::NetworkError { .. }
                | Self::ServerError { .. }
                | Self::RateLimited
                | Self::Timeout { .. }
        )
    }

    /// Returns true if the amount was too small for the swap provider.
    pub fn is_amount_too_low(&self) -> bool {
        matches!(self, Self::AmountTooLow { .. })
    }

    /// Classify an HTTP response from the 1-Click API.
    pub fn from_oneclick_response(status: u16, body: &str) -> Self {
        if body.contains("Amount is too low for bridge") {
            return Self::AmountTooLow {
                message: body.to_string(),
            };
        }

        if body.contains("Failed to get quote") {
            return Self::QuoteFailed {
                message: body.to_string(),
            };
        }

        match status {
            429 => Self::RateLimited,
            400..=499 => Self::ValidationError {
                message: body.to_string(),
            },
            500..=599 => Self::ServerError {
                status,
                message: body.to_string(),
            },
            _ => Self::Unknown {
                message: body.to_string(),
            },
        }
    }
}

impl std::fmt::Display for SwapErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AmountTooLow { message } => write!(f, "Amount too low: {message}"),
            Self::QuoteFailed { message } => write!(f, "Quote failed: {message}"),
            Self::NetworkError { message } => write!(f, "Network error: {message}"),
            Self::ServerError { status, message } => {
                write!(f, "Server error ({status}): {message}")
            }
            Self::RateLimited => write!(f, "Rate limited"),
            Self::ValidationError { message } => write!(f, "Validation error: {message}"),
            Self::Timeout { message } => write!(f, "Timeout: {message}"),
            Self::Unknown { message } => write!(f, "Unknown error: {message}"),
        }
    }
}

/// Swap error with classification and context.
#[derive(Debug)]
pub struct SwapError {
    /// Error classification
    pub kind: SwapErrorKind,
    /// Human-readable context (e.g. "Quote request", "Deposit")
    pub context: String,
}

impl SwapError {
    pub fn new(kind: SwapErrorKind, context: impl Into<String>) -> Self {
        Self {
            kind,
            context: context.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    pub fn is_amount_too_low(&self) -> bool {
        self.kind.is_amount_too_low()
    }
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.kind)
    }
}

impl std::error::Error for SwapError {}

/// Convert `SwapError` into `AppError` so it can flow through existing error paths.
impl From<SwapError> for AppError {
    fn from(err: SwapError) -> Self {
        AppError::ValidationError(err.to_string())
    }
}

/// Configuration for swap retry behaviour.
#[derive(Debug, Clone)]
pub struct SwapRetryConfig {
    /// Maximum number of attempts (including first try)
    pub max_attempts: u32,
    /// Base delay in milliseconds (doubles each attempt: 2s, 4s, 8s …)
    pub base_delay_ms: u64,
}

impl Default for SwapRetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 2000,
        }
    }
}

impl SwapRetryConfig {
    /// Calculate delay for a given attempt (1-indexed).
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let multiplier = 1u64 << attempt.saturating_sub(1); // 1, 2, 4, …
        Duration::from_millis(self.base_delay_ms * multiplier)
    }
}

/// Execute an async swap operation with retry logic for transient errors.
///
/// Only errors where `SwapError::is_retryable()` returns true are retried.
/// Non-retryable errors (amount-too-low, validation) are returned immediately.
///
/// # Errors
///
/// Returns the last `SwapError` (converted to `AppError`) if all retries are exhausted
/// or a non-retryable error is encountered.
pub async fn swap_with_retry<F, Fut>(
    config: &SwapRetryConfig,
    swap_name: &str,
    mut operation: F,
) -> AppResult<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), SwapError>>,
{
    let mut last_error: Option<SwapError> = None;

    for attempt in 1..=config.max_attempts {
        match operation().await {
            Ok(()) => return Ok(()),
            Err(e) if e.is_retryable() && attempt < config.max_attempts => {
                let delay = config.delay_for_attempt(attempt);
                tracing::debug!(
                    swap = %swap_name,
                    attempt,
                    max_attempts = config.max_attempts,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "Swap failed with retryable error, retrying"
                );
                sleep(delay).await;
                last_error = Some(e);
            }
            Err(e) => return Err(e.into()),
        }
    }

    // Should not normally reach here, but be safe
    Err(last_error
        .map_or_else(
            || {
                SwapError::new(
                    SwapErrorKind::Unknown {
                        message: "Retry loop exhausted".into(),
                    },
                    swap_name.to_string(),
                )
            },
            |e| e,
        )
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_classification() {
        assert!(SwapErrorKind::QuoteFailed {
            message: String::new()
        }
        .is_retryable());
        assert!(SwapErrorKind::NetworkError {
            message: String::new()
        }
        .is_retryable());
        assert!(SwapErrorKind::ServerError {
            status: 500,
            message: String::new()
        }
        .is_retryable());
        assert!(SwapErrorKind::RateLimited.is_retryable());
        assert!(SwapErrorKind::Timeout {
            message: String::new()
        }
        .is_retryable());

        // Not retryable
        assert!(!SwapErrorKind::AmountTooLow {
            message: String::new()
        }
        .is_retryable());
        assert!(!SwapErrorKind::ValidationError {
            message: String::new()
        }
        .is_retryable());
        assert!(!SwapErrorKind::Unknown {
            message: String::new()
        }
        .is_retryable());
    }

    #[test]
    fn test_amount_too_low_classification() {
        let kind = SwapErrorKind::from_oneclick_response(
            400,
            r#"{"message":"Amount is too low for bridge, try at least 10000"}"#,
        );
        assert!(kind.is_amount_too_low());
        assert!(!kind.is_retryable());
    }

    #[test]
    fn test_quote_failed_classification() {
        let kind =
            SwapErrorKind::from_oneclick_response(400, r#"{"message":"Failed to get quote"}"#);
        assert!(kind.is_retryable());
        assert!(!kind.is_amount_too_low());
    }

    #[test]
    fn test_server_error_classification() {
        let kind = SwapErrorKind::from_oneclick_response(500, "Internal Server Error");
        assert!(kind.is_retryable());
    }

    #[test]
    fn test_rate_limit_classification() {
        let kind = SwapErrorKind::from_oneclick_response(429, "Too Many Requests");
        assert!(kind.is_retryable());
        assert!(matches!(kind, SwapErrorKind::RateLimited));
    }

    #[test]
    fn test_retry_config_delay() {
        let config = SwapRetryConfig {
            max_attempts: 3,
            base_delay_ms: 2000,
        };
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(2000));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(4000));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(8000));
    }
}
