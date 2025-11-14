use std::{fmt::Display, ops::Deref, str::FromStr};

use near_sdk::{
    bs58, near,
    serde::{self, de, Deserialize, Serialize},
};

use crate::encoding::ParseError;

use super::PREFIX;

pub const SIGNATURE_LENGTH: usize = 64;

type ByteEncoding = [u8; SIGNATURE_LENGTH];

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh])]
pub struct Signature(pub ByteEncoding);

impl Deref for Signature {
    type Target = ByteEncoding;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<ByteEncoding> for Signature {
    fn as_ref(&self) -> &ByteEncoding {
        &self.0
    }
}

impl From<ByteEncoding> for Signature {
    fn from(value: ByteEncoding) -> Self {
        Self(value)
    }
}

impl From<Signature> for ByteEncoding {
    fn from(value: Signature) -> Self {
        value.0
    }
}

impl From<solana_sdk::signature::Signature> for Signature {
    fn from(value: solana_sdk::signature::Signature) -> Self {
        Self(value.into())
    }
}

impl From<Signature> for solana_sdk::signature::Signature {
    fn from(value: Signature) -> Self {
        Self::from(value.0)
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PREFIX}{}", bs58::encode(self.0).into_string())
    }
}

impl FromStr for Signature {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key_bs58 = s
            .strip_prefix(PREFIX)
            .ok_or(ParseError::MissingPrefix(PREFIX))?;
        let key_bytes = bs58::decode(key_bs58)
            .into_vec()
            .map_err(|e| ParseError::InvalidEncoding(e.into()))?;
        let len = key_bytes.len();
        let key = ByteEncoding::try_from(key_bytes).map_err(|_| ParseError::InvalidLength {
            expected: SIGNATURE_LENGTH,
            actual: len,
        })?;

        Ok(Self(key))
    }
}

impl schemars::JsonSchema for Signature {
    fn schema_name() -> String {
        "Signature".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("ED25519 signature".to_string());
        schema.string().pattern = Some("^ed25519:[1-9A-HJ-NP-Za-km-z]+$".to_string());
        schema.into()
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <&str as Deserialize>::deserialize(deserializer)?;
        Self::from_str(s).map_err(de::Error::custom)
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        <String as Serialize>::serialize(&self.to_string(), serializer)
    }
}
