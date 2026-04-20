#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("json serialization error: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid transaction hash: {0}")]
    InvalidTransactionHash(String),
    #[error("http request failed: {0}")]
    HttpRequest(String),
    #[error("near query failed: {0}")]
    NearQuery(String),
    #[error("unsupported signer account: {0}")]
    UnsupportedSignerAccount(String),
    #[error("near transaction failed: {0}")]
    NearTransaction(String),
    #[error("external service failed: {0}")]
    ExternalService(String),
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
