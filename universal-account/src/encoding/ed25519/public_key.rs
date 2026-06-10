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

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use solana_sdk::signer::{keypair::Keypair, Signer};

    use super::*;

    #[test]
    fn borsh_serialization() {
        let keypair = Keypair::new();
        let keypair_2 = Keypair::new();
        let pubkey = super::PublicKey(keypair.pubkey().to_bytes());
        let pubkey_2 = super::PublicKey(keypair_2.pubkey().to_bytes());

        assert_ne!(pubkey, pubkey_2);

        let borsh_ser = borsh::to_vec(&pubkey).unwrap();
        let borsh_ser_2 = borsh::to_vec(&pubkey_2).unwrap();

        assert_ne!(borsh_ser, borsh_ser_2);

        let parsed: super::PublicKey = borsh::from_slice(&borsh_ser).unwrap();
        let parsed_2: super::PublicKey = borsh::from_slice(&borsh_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(pubkey, parsed);
        assert_eq!(pubkey_2, parsed_2);
    }

    #[test]
    fn json_serialization() {
        let keypair = Keypair::new();
        let keypair_2 = Keypair::new();
        let pubkey = super::PublicKey(keypair.pubkey().to_bytes());
        let pubkey_2 = super::PublicKey(keypair_2.pubkey().to_bytes());

        assert_ne!(pubkey, pubkey_2);

        let json_ser = serde_json::to_string(&pubkey).unwrap();
        let json_ser_2 = serde_json::to_string(&pubkey_2).unwrap();

        assert_ne!(json_ser, json_ser_2);

        let parsed: super::PublicKey = serde_json::from_str(&json_ser).unwrap();
        let parsed_2: super::PublicKey = serde_json::from_str(&json_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(pubkey, parsed);
        assert_eq!(pubkey_2, parsed_2);
    }

    #[test]
    fn to_from_string() {
        let keypair = Keypair::new();
        let pubkey = super::PublicKey(keypair.pubkey().to_bytes());
        let pk_str = pubkey.to_string();

        let Some(b) = pk_str.strip_prefix("ed25519:") else {
            panic!("Missing prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 32, "Incorrect length");

        let parsed = super::PublicKey::from_str(&pk_str).unwrap();

        assert_eq!(parsed, pubkey);

        let keypair_2 = Keypair::new();
        let pk_str_2 = super::PublicKey(keypair_2.pubkey().to_bytes()).to_string();

        assert_ne!(pk_str, pk_str_2);
    }

    #[test]
    #[should_panic = r#"MissingPrefix("ed25519:")"#]
    fn from_string_err_prefix() {
        let s = "p256:QgbCYxWGboZy9VvWfAvdRs8M1EBtLabW9pPAZQP5UuNdz4gsY2EPG8xvmSAxyT8KMaFq677R3N5y8QmYSagCzFra";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidEncoding(InvalidCharacter { character: '*', index: 0 })"]
    fn from_string_err_bs58() {
        let s = "ed25519:*QgbCYxWGboZy9VvWfAvdRs8M1EBtLabW9pPAZQP5UuNdz4gsY2EPG8xvmSAxyT8KMaFq677R3N5y8QmYSagCzFra";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidLength { expected: 32, actual: 65 }"]
    fn from_string_err_length() {
        let s = "ed25519:QgbCYxWGboZy9VvWfAvdRs8M1EBtLabW9pPAZQP5UuNdz4gsY2EPG8xvmSAxyT8KMaFq677R3N5y8QmYSagCzFra";
        super::PublicKey::from_str(s).unwrap();
    }
}
