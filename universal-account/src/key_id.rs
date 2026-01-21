use near_sdk::near;

use crate::authentication::{
    ed25519::{eip191, raw, sep53},
    eip712,
    passkey::Passkey,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
    Ed25519RawKey(raw::VerifyKey),
    Eip712(eip712::VerifyKey),
    Sep53(sep53::VerifyKey),
    Eip191(eip191::VerifyKey),
}

impl std::fmt::Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passkey(key) => write!(f, "{}", key.0),
            Self::Ed25519RawKey(key) => write!(f, "{}", key.0),
            Self::Eip712(key) => write!(f, "{}", key.0),
            Self::Sep53(key) => write!(f, "{}", key.0),
            Self::Eip191(key) => write!(f, "{}", key.0),
        }
    }
}

impl From<Passkey> for KeyId {
    fn from(value: Passkey) -> Self {
        Self::Passkey(value)
    }
}

impl From<raw::VerifyKey> for KeyId {
    fn from(value: raw::VerifyKey) -> Self {
        Self::Ed25519RawKey(value)
    }
}

impl From<sep53::VerifyKey> for KeyId {
    fn from(value: sep53::VerifyKey) -> Self {
        Self::Sep53(value)
    }
}

impl From<eip712::VerifyKey> for KeyId {
    fn from(value: eip712::VerifyKey) -> Self {
        Self::Eip712(value)
    }
}

impl From<eip191::VerifyKey> for KeyId {
    fn from(value: eip191::VerifyKey) -> Self {
        Self::Eip191(value)
    }
}
