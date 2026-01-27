use near_sdk::near;

use crate::authentication::{
    ed25519::{eip191, raw, sep53},
    passkey,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(passkey::VerifyKey),
    #[serde(rename = "Ed25519RawKey")]
    Ed25519Raw(raw::VerifyKey),
    Sep53(sep53::VerifyKey),
    Eip191(eip191::VerifyKey),
}

impl std::fmt::Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passkey(key) => write!(f, "{}", key.0),
            Self::Ed25519Raw(key) => write!(f, "{}", key.0),
            Self::Sep53(key) => write!(f, "{}", key.0),
            Self::Eip191(key) => write!(f, "{}", key.0),
        }
    }
}

impl From<passkey::VerifyKey> for KeyId {
    fn from(value: passkey::VerifyKey) -> Self {
        Self::Passkey(value)
    }
}

impl From<raw::VerifyKey> for KeyId {
    fn from(value: raw::VerifyKey) -> Self {
        Self::Ed25519Raw(value)
    }
}

impl From<sep53::VerifyKey> for KeyId {
    fn from(value: sep53::VerifyKey) -> Self {
        Self::Sep53(value)
    }
}

impl From<eip191::VerifyKey> for KeyId {
    fn from(value: eip191::VerifyKey) -> Self {
        Self::Eip191(value)
    }
}
