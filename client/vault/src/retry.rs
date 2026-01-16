//! Retry utilities with exponential backoff for NEAR RPC calls.

use std::time::Duration;

use crate::RetryConfig;

/// Determines if an error is retryable (timeout or I/O error).
pub fn should_retry(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.is::<tokio::time::error::Elapsed>() {
            return true;
        }
        if cause.is::<std::io::Error>() {
            return true;
        }
    }
    false
}

/// Manages retry state with exponential backoff.
///
/// Usage:
/// ```ignore
/// let mut retry_state = RetryState::new(config);
/// loop {
///     match operation().await {
///         Ok(result) => return Ok(result),
///         Err(e) => {
///             if !retry_state.should_retry_err(&e).await {
///                 return Err(e);
///             }
///             // continues to next iteration
///         }
///     }
/// }
/// ```
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
            attempts_left: config.map(|r| r.max_attempts).unwrap_or(1),
            backoff_ms: config.map(|r| r.initial_backoff_ms).unwrap_or(0),
            max_backoff_ms: config.map(|r| r.max_backoff_ms).unwrap_or(0),
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
}
