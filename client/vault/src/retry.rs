//! Retry utilities with exponential backoff for NEAR RPC calls.

use std::time::Duration;

use near_jsonrpc_client::errors::JsonRpcError;
use near_jsonrpc_primitives::types::{query::RpcQueryError, transactions::RpcTransactionError};

use crate::RetryConfig;

/// Determines if an error is retryable (timeout or I/O error).
pub fn should_retry(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.is::<tokio::time::error::Elapsed>()
            || cause.is::<std::io::Error>()
            || cause.is::<serde_json::Error>()
            || cause
                .downcast_ref::<JsonRpcError<RpcQueryError>>()
                .is_some()
            || cause
                .downcast_ref::<JsonRpcError<RpcTransactionError>>()
                .is_some()
    })
}

/// Manages retry state with exponential backoff.
pub struct RetryState {
    attempts_left: u32,
    backoff_ms: u64,
    max_backoff_ms: u64,
}

impl RetryState {
    /// Create a new retry state from an optional config.
    ///
    /// If config is None, allows exactly 1 attempt with no retry.
    pub fn new(config: Option<RetryConfig>) -> Self {
        let config = config.map(|c| c.normalized());
        Self {
            attempts_left: config.map_or(1, |r| r.max_attempts),
            backoff_ms: config.map_or(0, |r| r.initial_backoff_ms),
            max_backoff_ms: config.map_or(0, |r| r.max_backoff_ms),
        }
    }

    /// Decrement attempts counter. Call this at the start of each attempt.
    pub fn begin_attempt(&mut self) {
        self.attempts_left = self.attempts_left.saturating_sub(1);
    }

    /// Check if we should retry after an error, and if so, sleep for backoff.
    ///
    /// Returns `true` if the caller should continue retrying.
    /// Returns `false` if we've exhausted retries or the error is not retryable.
    pub async fn should_retry_err(&mut self, err: &anyhow::Error) -> bool {
        if self.attempts_left == 0 || !should_retry(err) {
            return false;
        }
        self.sleep_and_backoff().await;
        true
    }

    /// Sleep for current backoff duration and increase backoff for next attempt.
    async fn sleep_and_backoff(&mut self) {
        tokio::time::sleep(Duration::from_millis(self.backoff_ms)).await;
        self.backoff_ms = (self.backoff_ms.saturating_mul(2)).min(self.max_backoff_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_state_no_config() {
        let state = RetryState::new(None);
        assert_eq!(state.attempts_left, 1);
        assert_eq!(state.backoff_ms, 0);
    }

    #[test]
    fn retry_state_with_config() {
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 1000,
        };
        let state = RetryState::new(Some(config));
        assert_eq!(state.attempts_left, 3);
        assert_eq!(state.backoff_ms, 100);
        assert_eq!(state.max_backoff_ms, 1000);
    }

    #[test]
    fn should_retry_io_error() {
        let err = anyhow::Error::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        ));
        assert!(should_retry(&err));
    }

    #[test]
    fn should_retry_other_error() {
        let err = anyhow::anyhow!("some other error");
        assert!(!should_retry(&err));
    }

    #[test]
    fn should_retry_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("not-json").unwrap_err();
        let err = anyhow::Error::new(serde_err);
        assert!(should_retry(&err));
    }
}
