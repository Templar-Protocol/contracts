//! Error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Market error: {0}")]
    Market(String),

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, MonitorError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_rpc() {
        let err = MonitorError::Rpc("connection failed".to_string());
        assert_eq!(err.to_string(), "RPC error: connection failed");
    }

    #[test]
    fn test_error_display_config() {
        let err = MonitorError::Config("missing env var".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing env var");
    }

    #[test]
    fn test_error_display_market() {
        let err = MonitorError::Market("invalid position".to_string());
        assert_eq!(err.to_string(), "Market error: invalid position");
    }

    #[test]
    fn test_error_display_telegram() {
        let err = MonitorError::Telegram("send failed".to_string());
        assert_eq!(err.to_string(), "Telegram error: send failed");
    }

    #[test]
    fn test_error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json")
            .unwrap_err();
        let monitor_err: MonitorError = json_err.into();
        assert!(monitor_err.to_string().contains("JSON error"));
    }

    #[test]
    fn test_result_type_ok() {
        let result: Result<i32> = Ok(42);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_result_type_err() {
        let result: Result<i32> = Err(MonitorError::Rpc("test".to_string()));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.to_string(), "RPC error: test");
    }
}
