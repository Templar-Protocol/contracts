use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::serde::{de, Deserialize, Serialize};
use near_sdk::{bs58, near};
use p256::ecdsa::{signature, VerifyingKey};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [])]
pub struct PublicKey(pub VerifyingKey);

impl Deref for PublicKey {
    type Target = VerifyingKey;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("Missing \"p256:\" prefix")]
    MissingPrefix,
    #[error("Invalid base58: {0}")]
    InvalidBase58(#[from] bs58::decode::Error),
    #[error("Invalid key data: {0}")]
    InvalidKeyData(#[from] signature::Error),
}

impl Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.0.to_sec1_bytes();
        write!(f, "p256:{}", bs58::encode(bytes).into_string())
    }
}

impl FromStr for PublicKey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key_bs58 = s.strip_prefix("p256:").ok_or(ParseError::MissingPrefix)?;
        let key_bytes = bs58::decode(key_bs58).into_vec()?;
        let key = VerifyingKey::from_sec1_bytes(&key_bytes)?;

        Ok(Self(key))
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        let s = <&str as Deserialize>::deserialize(deserializer)?;
        Self::from_str(s).map_err(de::Error::custom)
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        <String as Serialize>::serialize(&self.to_string(), serializer)
    }
}

impl BorshSerialize for PublicKey {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let bytes = self.0.to_sec1_bytes();
        BorshSerialize::serialize(&bytes, writer)
    }
}

impl BorshDeserialize for PublicKey {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Ok(Self(
            VerifyingKey::from_sec1_bytes(&<Box<[u8]> as BorshDeserialize>::deserialize_reader(
                reader,
            )?)
            .map_err(|e| std::io::Error::other(e.to_string()))?,
        ))
    }
}
