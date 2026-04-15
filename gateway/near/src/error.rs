#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid transaction hash: {0}")]
    InvalidTransactionHash(String),
    #[error("near query failed: {0}")]
    NearQuery(String),
    #[error("unsupported signer account: {0}")]
    UnsupportedSignerAccount(String),
    #[error("near transaction failed: {0}")]
    NearTransaction(String),
    #[error("actor unavailable: {0}")]
    ActorUnavailable(&'static str),
    #[error("actor error ({actor}): {source}")]
    ActorError {
        actor: &'static str,
        #[source]
        source: actix::MailboxError,
    },
}

pub type GatewayResult<T> = Result<T, GatewayError>;
