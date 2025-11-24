use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Address, U256},
};
use near_sdk::serde::{Deserialize, Serialize};
use schemars::schema_for_value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Domain(pub Eip712Domain);

impl From<Eip712Domain> for Domain {
    fn from(value: Eip712Domain) -> Self {
        Self(value)
    }
}

impl From<Domain> for Eip712Domain {
    fn from(value: Domain) -> Self {
        value.0
    }
}

impl Serialize for Domain {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Domain {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        Ok(Self(Eip712Domain::deserialize(deserializer)?))
    }
}

impl schemars::JsonSchema for Domain {
    fn schema_name() -> String {
        "Domain".to_string()
    }

    fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        schema_for_value!(Eip712Domain {
            name: Some("name".into()),
            version: Some("1".into()),
            chain_id: Some(U256::from(1u8)),
            verifying_contract: Some(Address([0u8; 20].into())),
            salt: Some([0xffu8; 32].into()),
        })
        .schema
        .into()
    }
}
