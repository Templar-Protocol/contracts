use near_sdk::near;

use crate::authentication::{ed25519::raw, eip712, passkey::Passkey};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum KeyId {
    Passkey(Passkey),
    Ed25519RawKey(raw::VerifyKey),
    Eip712(eip712::VerifyKey),
}

impl std::fmt::Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passkey(passkey) => write!(f, "{}", passkey.0),
            Self::Ed25519RawKey(ed25519_raw_key) => write!(f, "{}", ed25519_raw_key.0),
            Self::Eip712(key) => write!(f, "{}", key.0),
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

impl From<eip712::VerifyKey> for KeyId {
    fn from(value: eip712::VerifyKey) -> Self {
        Self::Eip712(value)
    }
}
