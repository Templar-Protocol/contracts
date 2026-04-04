#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid transaction hash: {0}")]
    InvalidTransactionHash(String),
    #[error("near query failed: {0}")]
    NearQuery(String),
}

pub type GatewayResult<T> = Result<T, GatewayError>;
