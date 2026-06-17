use thiserror::Error;

/// Reasons a Pyth Pro update may be rejected during verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerifyError {
    /// The outer signed-message envelope could not be decoded.
    #[error("failed to decode Lazer message envelope: {0}")]
    Envelope(String),
    /// The inner price payload could not be decoded.
    #[error("failed to decode payload: {0}")]
    Payload(String),
    /// The message or payload was not canonically encoded (e.g. trailing bytes), so it does not
    /// re-serialize to the exact signed/received bytes.
    #[error("message is not canonically encoded")]
    NonCanonical,
    /// The ed25519 signature did not verify against the carried public key.
    #[error("ed25519 signature verification failed")]
    Signature,
    /// The signer is not in the trusted set, or its trust has expired.
    #[error("signer is not trusted or has expired")]
    UntrustedSigner,
    /// The payload was published on a channel the adapter does not accept.
    #[error("payload channel {got} is not accepted")]
    Channel { got: u8 },
    /// The payload timestamp is older than the configured freshness window allows.
    #[error("payload timestamp is too old")]
    TimestampTooOld,
    /// The payload timestamp is further in the future than the configured window allows.
    #[error("payload timestamp is too far in the future")]
    TimestampTooFarAhead,
}
