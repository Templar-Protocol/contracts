use std::{ops::Deref, str::FromStr};

use near_sdk::{
    base64::prelude::*,
    near,
    serde::{self, de, Deserialize, Serialize},
};

#[derive(Clone, Debug)]
#[near(serializers = [borsh])]
pub struct Challenge(pub [u8; 32]);

impl Deref for Challenge {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for Challenge {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 32]> for Challenge {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<Challenge> for [u8; 32] {
    fn from(value: Challenge) -> Self {
        value.0
    }
}

impl<'de> Deserialize<'de> for Challenge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes_b64url: String = Deserialize::deserialize(deserializer)?;
        let mut buf = [0u8; 32];
        let decoded_bytes_len = BASE64_URL_SAFE_NO_PAD
            .decode_slice(&bytes_b64url, &mut buf)
            .map_err(serde::de::Error::custom)?;
        if decoded_bytes_len != 32 {
            return Err(serde::de::Error::custom("expected 32 bytes"));
        }
        Ok(Self(buf))
    }
}

impl Serialize for Challenge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&BASE64_URL_SAFE_NO_PAD.encode(self.0), serializer)
    }
}

impl schemars::JsonSchema for Challenge {
    fn schema_name() -> String {
        "Challenge".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Base64url-encoded SHA-256 challenge".to_string());
        schema.string().pattern = Some("^[A-Za-z0-9_-]+$".to_string());
        schema.into()
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
#[serde(rename_all = "camelCase")]
pub struct ClientDataJson {
    pub r#type: String,
    pub challenge: Challenge,
    pub origin: String,
    pub cross_origin: Option<bool>,
    pub top_origin: Option<String>,
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct AuthenticatorData(pub Box<[u8]>);

impl FromStr for AuthenticatorData {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s)?;
        Ok(Self(bytes.into_boxed_slice()))
    }
}

impl Deref for AuthenticatorData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for AuthenticatorData {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthenticatorData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes_hex: String = Deserialize::deserialize(deserializer)?;
        Self::from_str(&bytes_hex).map_err(de::Error::custom)
    }
}

impl Serialize for AuthenticatorData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&hex::encode(&self.0), serializer)
    }
}

impl schemars::JsonSchema for AuthenticatorData {
    fn schema_name() -> String {
        "AuthenticatorData".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Hex-encoded passkey authenticator data".to_string());
        schema.string().pattern = Some("^[A-Fa-f0-9]+$".to_string());
        schema.into()
    }
}
