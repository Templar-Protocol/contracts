use std::collections::BTreeMap;
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use near_sdk::borsh::{self, BorshDeserialize, BorshSchema, BorshSerialize};
use near_sdk::serde::{self, de, Deserialize, Serialize};
use near_sdk::{bs58, near};

use super::ParseError;

pub static PREFIX: &str = "p256:";
const KEY_LENGTH: usize = 65;
type ByteEncoding = [u8; KEY_LENGTH];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [])]
pub struct PublicKey(pub p256::PublicKey);

impl From<PublicKey> for p256::PublicKey {
    fn from(value: PublicKey) -> Self {
        value.0
    }
}

impl From<p256::PublicKey> for PublicKey {
    fn from(value: p256::PublicKey) -> Self {
        Self(value)
    }
}

impl Deref for PublicKey {
    type Target = p256::PublicKey;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.0.to_sec1_bytes();
        write!(f, "{PREFIX}{}", bs58::encode(bytes).into_string())
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
        let actual = key_bytes.len();
        if actual != KEY_LENGTH {
            return Err(ParseError::InvalidLength {
                expected: KEY_LENGTH,
                actual,
            });
        }
        let key = p256::PublicKey::from_sec1_bytes(&key_bytes)
            .map_err(|e| ParseError::InvalidEncoding(e.into()))?;

        Ok(Self(key))
    }
}

impl schemars::JsonSchema for PublicKey {
    fn schema_name() -> String {
        "PublicKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("NIST P256 public key".to_string());
        schema.string().pattern = Some("^p256:[1-9A-HJ-NP-Za-km-z]{88,89}$".to_string());
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

impl BorshSchema for PublicKey {
    fn add_definitions_recursively(
        definitions: &mut BTreeMap<borsh::schema::Declaration, borsh::schema::Definition>,
    ) {
        <ByteEncoding as BorshSchema>::add_definitions_recursively(definitions);
    }

    fn declaration() -> borsh::schema::Declaration {
        String::from("PublicKey")
    }
}

impl BorshSerialize for PublicKey {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        #[allow(clippy::unwrap_used, reason = "Key length is known")]
        let bytes: ByteEncoding = (&*self.0.to_sec1_bytes()).try_into().unwrap();
        BorshSerialize::serialize(&bytes, writer)
    }
}

impl BorshDeserialize for PublicKey {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Ok(Self(
            p256::PublicKey::from_sec1_bytes(
                &<ByteEncoding as BorshDeserialize>::deserialize_reader(reader)?,
            )
            .map_err(|e| std::io::Error::other(e.to_string()))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use p256::elliptic_curve::rand_core::OsRng;

    use super::*;

    #[test]
    fn borsh_serialization() {
        let key = p256::SecretKey::random(&mut OsRng);
        let key_2 = p256::SecretKey::random(&mut OsRng);
        let pubkey = super::PublicKey::from(key.public_key());
        let pubkey_2 = super::PublicKey::from(key_2.public_key());

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
        let key = p256::SecretKey::random(&mut OsRng);
        let key_2 = p256::SecretKey::random(&mut OsRng);
        let pubkey = super::PublicKey::from(key.public_key());
        let pubkey_2 = super::PublicKey::from(key_2.public_key());

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
        let key = p256::SecretKey::random(&mut OsRng);
        let pubkey = super::PublicKey::from(key.public_key());
        let pk_str = pubkey.to_string();

        let Some(b) = pk_str.strip_prefix("p256:") else {
            panic!("Missing prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 65, "Incorrect length");

        let parsed = super::PublicKey::from_str(&pk_str).unwrap();

        assert_eq!(parsed, pubkey);

        let key_2 = p256::SecretKey::random(&mut OsRng);
        let pk_str_2 = super::PublicKey::from(key_2.public_key()).to_string();

        assert_ne!(pk_str, pk_str_2);
    }

    #[test]
    #[should_panic = r#"MissingPrefix("p256:")"#]
    fn from_string_err_prefix() {
        let s = "ed25519:5dZgMshSoMVwebufwFJm8pWyNqrY8VxMCsgFrKfe3KRc";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidEncoding(InvalidCharacter { character: '*', index: 0 })"]
    fn from_string_err_bs58() {
        let s = "p256:*5dZgMshSoMVwebufwFJm8pWyNqrY8VxMCsgFrKfe3KRc";
        super::PublicKey::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidLength { expected: 65, actual: 32 }"]
    fn from_string_err_length() {
        let s = "p256:5dZgMshSoMVwebufwFJm8pWyNqrY8VxMCsgFrKfe3KRc";
        super::PublicKey::from_str(s).unwrap();
    }
}
