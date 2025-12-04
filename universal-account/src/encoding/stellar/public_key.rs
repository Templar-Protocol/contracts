use std::{ops::Deref, str::FromStr};

use near_sdk::{near, serde};
use stellar_strkey::ed25519::PublicKey as StellarPublicKey;

type ByteEncoding = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh])]
pub struct PublicKey(pub ByteEncoding);

impl PublicKey {
    pub fn verify(&self, message: &[u8], signature: &crate::encoding::ed25519::Signature) -> bool {
        near_sdk::env::ed25519_verify(signature.as_ref(), message, &self.0)
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&StellarPublicKey(self.0), f)
    }
}

impl Deref for PublicKey {
    type Target = ByteEncoding;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<StellarPublicKey> for PublicKey {
    fn from(value: StellarPublicKey) -> Self {
        Self(value.0)
    }
}

impl From<PublicKey> for StellarPublicKey {
    fn from(value: PublicKey) -> Self {
        StellarPublicKey(value.0)
    }
}

impl From<ByteEncoding> for PublicKey {
    fn from(value: ByteEncoding) -> Self {
        Self(value)
    }
}

impl From<PublicKey> for ByteEncoding {
    fn from(value: PublicKey) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<ByteEncoding> for PublicKey {
    fn as_ref(&self) -> &ByteEncoding {
        &self.0
    }
}

impl FromStr for PublicKey {
    type Err = <StellarPublicKey as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(StellarPublicKey::from_str(s)?.0))
    }
}

impl serde::Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        StellarPublicKey(self.0).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(StellarPublicKey::deserialize(deserializer)?.0))
    }
}

impl schemars::JsonSchema for PublicKey {
    fn schema_name() -> String {
        "PublicKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Stellar public key".to_string());
        schema.string().pattern = Some("^G[A-Z2-7]{55}$".to_string());
        schema.into()
    }
}
