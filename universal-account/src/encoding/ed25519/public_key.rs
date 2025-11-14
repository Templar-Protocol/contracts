use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use near_sdk::serde::{self, de, Deserialize, Serialize};
use near_sdk::{bs58, near};

use crate::encoding::ParseError;

use super::PREFIX;

pub const KEY_LENGTH: usize = 32;

type ByteEncoding = [u8; KEY_LENGTH];

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh])]
pub struct PublicKey(pub ByteEncoding);

impl PublicKey {
    pub fn verify(&self, message: &[u8], signature: &super::Signature) -> bool {
        // #[cfg(target_arch = "wasm32")]
        near_sdk::env::ed25519_verify(signature.as_ref(), message, &self.0)
    }
}

impl From<PublicKey> for ByteEncoding {
    fn from(value: PublicKey) -> Self {
        value.0
    }
}

impl From<ByteEncoding> for PublicKey {
    fn from(value: ByteEncoding) -> Self {
        Self(value)
    }
}

impl Deref for PublicKey {
    type Target = ByteEncoding;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PREFIX}{}", bs58::encode(self.0).into_string())
    }
}

impl FromStr for PublicKey {
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
            expected: KEY_LENGTH,
            actual: len,
        })?;

        Ok(Self(key))
    }
}

impl schemars::JsonSchema for PublicKey {
    fn schema_name() -> String {
        "PublicKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("ED25519 public key".to_string());
        schema.string().pattern = Some("^ed25519:[1-9A-HJ-NP-Za-km-z]+$".to_string());
        schema.into()
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <&str as Deserialize>::deserialize(deserializer)?;
        Self::from_str(s).map_err(de::Error::custom)
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        <String as Serialize>::serialize(&self.to_string(), serializer)
    }
}

// impl BorshSchema for PublicKey {
//     fn add_definitions_recursively(
//         definitions: &mut BTreeMap<borsh::schema::Declaration, borsh::schema::Definition>,
//     ) {
//         <ByteEncoding as BorshSchema>::add_definitions_recursively(definitions);
//     }

//     fn declaration() -> borsh::schema::Declaration {
//         String::from("PublicKey")
//     }
// }

// impl BorshSerialize for PublicKey {
//     fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
//         BorshSerialize::serialize(&self.0, writer)
//     }
// }

// impl BorshDeserialize for PublicKey {
//     fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
//         Ok(Self(
//             <ByteEncoding as BorshDeserialize>::deserialize_reader(reader)?,
//         ))
//     }
// }
