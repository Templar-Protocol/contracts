use std::ops::Deref;

use near_sdk::{
    base64::prelude::*,
    near,
    serde::{self, de, Deserialize, Serialize},
};
use p256::ecdsa;

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct Signature(pub ecdsa::DerSignature);

impl Deref for Signature {
    type Target = ecdsa::DerSignature;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let signature_b64url: String = serde::Deserialize::deserialize(deserializer)?;
        let signature_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(signature_b64url)
            .map_err(de::Error::custom)?;
        let signature =
            ecdsa::DerSignature::try_from(signature_bytes.as_slice()).map_err(de::Error::custom)?;
        Ok(Self(signature))
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let signature_b64url = BASE64_URL_SAFE_NO_PAD.encode(self.0.as_bytes());
        Serialize::serialize(&signature_b64url, serializer)
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
