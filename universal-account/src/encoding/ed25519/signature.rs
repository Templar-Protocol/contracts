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

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use solana_sdk::signer::{keypair::Keypair, Signer};

    use super::*;

    #[test]
    fn borsh_serialization() {
        let keypair = Keypair::new();
        let signature =
            super::Signature::from(*keypair.sign_message(b"borsh serialization").as_array());
        let signature_2 =
            super::Signature::from(*keypair.sign_message(b"borsh serialization 2").as_array());

        assert_ne!(signature, signature_2);

        let borsh_ser = borsh::to_vec(&signature).unwrap();
        let borsh_ser_2 = borsh::to_vec(&signature_2).unwrap();

        assert_ne!(borsh_ser, borsh_ser_2);

        let parsed: super::Signature = borsh::from_slice(&borsh_ser).unwrap();
        let parsed_2: super::Signature = borsh::from_slice(&borsh_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(signature, parsed);
        assert_eq!(signature_2, parsed_2);
    }

    #[test]
    fn json_serialization() {
        let keypair = Keypair::new();
        let signature =
            super::Signature::from(*keypair.sign_message(b"json serialization").as_array());
        let signature_2 =
            super::Signature::from(*keypair.sign_message(b"json serialization 2").as_array());

        assert_ne!(signature, signature_2);

        let json_ser = serde_json::to_string(&signature).unwrap();
        let json_ser_2 = serde_json::to_string(&signature_2).unwrap();

        assert_ne!(json_ser, json_ser_2);

        let parsed: super::Signature = serde_json::from_str(&json_ser).unwrap();
        let parsed_2: super::Signature = serde_json::from_str(&json_ser_2).unwrap();

        assert_ne!(parsed, parsed_2);

        assert_eq!(signature, parsed);
        assert_eq!(signature_2, parsed_2);
    }

    #[test]
    fn to_from_string() {
        let keypair = Keypair::new();
        let signature = super::Signature::from(
            *keypair
                .sign_message(b"test ToString/FromStr implementation")
                .as_array(),
        );
        let sig_str = signature.to_string();

        let Some(b) = sig_str.strip_prefix("ed25519:") else {
            panic!("Missing prefix");
        };

        let b = bs58::decode(b).into_vec().unwrap();
        assert_eq!(b.len(), 64, "Incorrect length");

        let parsed = super::Signature::from_str(&sig_str).unwrap();

        assert_eq!(parsed, signature);

        let sig_str_2 =
            super::Signature::from(*keypair.sign_message(b"A different message").as_array())
                .to_string();

        assert_ne!(sig_str, sig_str_2);
    }

    #[test]
    #[should_panic = r#"MissingPrefix("ed25519:")"#]
    fn from_string_err_prefix() {
        let s = "wC3KDXXriL2HFPztgvpcWES2bBzaDBWV2xY5rwrXTFRMmM59p434FLYfZZTu2iSLdu99wcWuGnva5yHQSaCZsJW";
        super::Signature::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidEncoding(InvalidCharacter { character: '*', index: 0 })"]
    fn from_string_err_bs58() {
        let s = "ed25519:*wC3KDXXriL2HFPztgvpcWES2bBzaDBWV2xY5rwrXTFRMmM59p434FLYfZZTu2iSLdu99wcWuGnva5yHQSaCZsJW";
        super::Signature::from_str(s).unwrap();
    }

    #[test]
    #[should_panic = "InvalidLength { expected: 64, actual: 65 }"]
    fn from_string_err_length() {
        let s = "ed25519:wC3KDXXriL2HFPztgvpcWES2bBzaDBWV2xY5rwrXTFRMmM59p434FLYfZZTu2iSLdu99wcWuGnva5yHQSaCZsJWa";
        super::Signature::from_str(s).unwrap();
    }
}
