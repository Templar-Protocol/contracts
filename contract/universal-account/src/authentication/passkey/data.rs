use std::ops::Deref;

use near_sdk::{
    near,
    serde::{self, de, Deserialize, Serialize},
};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
#[serde(rename_all = "camelCase")]
pub struct ClientDataJson {
    pub r#type: String,
    pub challenge: String,
    pub origin: String,
    pub cross_origin: Option<bool>,
    pub top_origin: Option<String>,
}

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct AuthenticatorData(pub Box<[u8]>);

impl Deref for AuthenticatorData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthenticatorData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes_hex: String = Deserialize::deserialize(deserializer)?;
        let bytes = hex::decode(bytes_hex).map_err(de::Error::custom)?;
        Ok(Self(bytes.into_boxed_slice()))
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
