use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use near_account_id::AccountId;
pub use near_gas::NearGas;
pub use near_token::NearToken;
use schemars::{
    gen::SchemaGenerator,
    schema::{InstanceType, Metadata, Schema, SchemaObject, StringValidation},
    JsonSchema,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::macros::transparent_newtype;

transparent_newtype!(
    pub struct ManagedAccountId(AccountId);
);
transparent_newtype!(
    pub struct RegistryId(AccountId);
);
transparent_newtype!(
    pub struct MarketId(AccountId);
);
transparent_newtype!(
    pub struct UniversalAccountId(AccountId);
);
transparent_newtype!(
    pub struct ContractMethodName(String);
);
transparent_newtype!(
    pub struct IdempotencyKey(String);
);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Base64Bytes(pub Vec<u8>);

impl Serialize for Base64Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64_STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = BASE64_STANDARD.decode(&encoded).map_err(D::Error::custom)?;
        Ok(Self(bytes))
    }
}

impl JsonSchema for Base64Bytes {
    fn schema_name() -> String {
        "Base64Bytes".to_owned()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(InstanceType::String.into()),
            string: Some(Box::new(StringValidation::default())),
            metadata: Some(Box::new(Metadata {
                title: Some("Base64 Bytes".to_owned()),
                description: Some("Base64-encoded binary payload.".to_owned()),
                ..Metadata::default()
            })),
            format: Some("byte".to_owned()),
            ..SchemaObject::default()
        })
    }
}
