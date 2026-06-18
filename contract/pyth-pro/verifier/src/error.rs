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
    /// Decoding the message or payload left trailing bytes — the cursor did not reach the end of
    /// the input. This rejects trailing / non-exhaustive encodings only; it does **not**
    /// re-serialize and byte-compare, so an in-band non-canonical encoding that still parses and
    /// fully consumes its input is not caught here (any such mutation would instead fail the
    /// ed25519 signature check, which is computed over the exact signed bytes).
    #[error("message or payload has trailing bytes (non-exhaustive encoding)")]
    TrailingBytes,
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
