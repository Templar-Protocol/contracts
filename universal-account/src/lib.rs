use authentication::{
    ed25519_raw::Ed25519RawKey, passkey::Passkey, ExecutionContextProvider, ExecutionError,
    InvalidSignatureError, Key,
};
use near_sdk::{json_types::U64, near, AccountIdRef};

pub mod authentication;
pub mod encoding;
mod event;
pub use event::Event;
use transaction::Transaction;
pub mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct InitArgs {
    pub key: KeyId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
    Ed25519RawKey(Ed25519RawKey),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(deny_unknown_fields)]
pub struct ExecutionParameters {
    /// Static. If a universal account is deleted and recreated with the same
    /// keys, this ensures that old signatures are not replayable.
    pub block_height: U64,
    /// Static. If a key is deleted and re-added to the same account, this
    /// ensures that that old signatures are not replayable.
    pub index: U64,
    /// Increments for each message executed by this key.
    pub nonce: U64,
}

impl ExecutionParameters {
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            nonce: U64(self.nonce.0 + 1),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone)]
#[near(serializers = [json])]
pub enum ExecuteArgs {
    Passkey {
        key: Passkey,
        message: authentication::passkey::MessageWithSignature<Box<[transaction::Transaction]>>,
    },
    Ed25519Raw {
        key: Ed25519RawKey,
        message: authentication::ed25519_raw::MessageWithSignature<Box<[transaction::Transaction]>>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error(transparent)]
    Signature(#[from] InvalidSignatureError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
}

impl ExecuteArgs {
    pub fn key(&self) -> KeyId {
        match self {
            ExecuteArgs::Passkey { key, .. } => KeyId::Passkey(key.clone()),
            ExecuteArgs::Ed25519Raw { key, .. } => KeyId::Ed25519RawKey(key.clone()),
        }
    }

    /// # Errors
    ///
    /// - If signature verification fails
    /// - If execution parameters do not match
    pub fn verify(
        self,
        executor_account_id: &AccountIdRef,
        parameters: &ExecutionParameters,
        allowed_origin: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<Box<[Transaction]>, VerificationError> {
        Ok(match self {
            ExecuteArgs::Passkey { key, message } => key
                .verify_signature(message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
            ExecuteArgs::Ed25519Raw { key, message } => key
                .verify_signature(message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
        })
    }
}
