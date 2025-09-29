use std::{fmt::Display, ops::Deref, str::FromStr};

use near_sdk::{
    base64::prelude::*,
    near,
    serde::{self, de, Deserialize, Serialize},
};
use p256::ecdsa;

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct Signature(pub ecdsa::DerSignature);

impl From<ecdsa::DerSignature> for Signature {
    fn from(value: ecdsa::DerSignature) -> Self {
        Self(value)
    }
}

impl From<Signature> for ecdsa::DerSignature {
    fn from(value: Signature) -> Self {
        value.0
    }
}

impl Deref for Signature {
    type Target = ecdsa::DerSignature;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Base64(#[from] near_sdk::base64::DecodeError),
    #[error(transparent)]
    Signature(#[from] ecdsa::signature::Error),
}

impl FromStr for Signature {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let signature_bytes = BASE64_URL_SAFE_NO_PAD.decode(s)?;
        let signature = ecdsa::DerSignature::try_from(signature_bytes.as_slice())?;
        Ok(Self(signature))
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", BASE64_URL_SAFE_NO_PAD.encode(self.0.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        Self::from_str(&s).map_err(de::Error::custom)
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.to_string(), serializer)
    }
}

impl schemars::JsonSchema for Signature {
    fn schema_name() -> String {
        "Signature".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Base64URL-encoded NIST P256 signature".to_string());
        schema.string().pattern = Some("^[A-Za-z0-9_-]{,72}$".to_string());
        schema.into()
    }
}
