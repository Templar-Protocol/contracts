use std::fmt::Display;

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

impl Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyId::Passkey(passkey) => write!(f, "passkey:{}", passkey.0),
            KeyId::Ed25519RawKey(ed25519_raw_key) => write!(f, "{}", ed25519_raw_key.0),
        }
    }
}

impl From<Passkey> for KeyId {
    fn from(value: Passkey) -> Self {
        Self::Passkey(value)
    }
}

impl From<Ed25519RawKey> for KeyId {
    fn from(value: Ed25519RawKey) -> Self {
        Self::Ed25519RawKey(value)
    }
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
        message:
            Box<authentication::passkey::MessageWithSignature<Box<[transaction::Transaction]>>>,
    },
    Ed25519Raw {
        key: Ed25519RawKey,
        message:
            Box<authentication::ed25519_raw::MessageWithSignature<Box<[transaction::Transaction]>>>,
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
    pub fn key_id(&self) -> KeyId {
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
                .verify_signature(*message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
            ExecuteArgs::Ed25519Raw { key, message } => key
                .verify_signature(*message)?
                .verify_execution(executor_account_id, parameters, allowed_origin)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::bs58;
    use p256::elliptic_curve::rand_core::OsRng;
    use solana_sdk::{signature::Keypair, signer::Signer};

    use super::*;

    #[test]
    fn keyid_serialization() {
        let sk_p256 = p256::SecretKey::random(&mut OsRng);
        let passkey = Passkey(sk_p256.public_key().into());
        let passkey_id: KeyId = passkey.into();
        let passkey_id_str = passkey_id.to_string();

        let Some(b) = passkey_id_str.strip_prefix("passkey:p256:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 65);

        let sk_ed25519 = Keypair::new();
        let ed25519_raw = Ed25519RawKey(sk_ed25519.pubkey().to_bytes().into());
        let ed25519_raw_id: KeyId = ed25519_raw.into();
        let ed25519_raw_id_str = ed25519_raw_id.to_string();

        let Some(b) = ed25519_raw_id_str.strip_prefix("ed25519:") else {
            panic!("invalid prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 32);
    }
}
