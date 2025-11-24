use std::collections::BTreeMap;

use alloy::primitives::Address as AlloyAddress;
use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{borsh, near};
use schemars::JsonSchema;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [])]
pub struct Address(pub AlloyAddress);

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<AlloyAddress> for Address {
    fn from(value: AlloyAddress) -> Self {
        Self(value)
    }
}

impl From<Address> for AlloyAddress {
    fn from(value: Address) -> Self {
        value.0
    }
}

impl From<[u8; 20]> for Address {
    fn from(value: [u8; 20]) -> Self {
        Self(AlloyAddress::from(value))
    }
}

impl From<Address> for [u8; 20] {
    fn from(value: Address) -> Self {
        value.0 .0 .0
    }
}

impl AsRef<[u8; 20]> for Address {
    fn as_ref(&self) -> &[u8; 20] {
        &self.0 .0
    }
}

impl Serialize for Address {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        AlloyAddress::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        Ok(Self(AlloyAddress::deserialize(deserializer)?))
    }
}

impl JsonSchema for Address {
    fn schema_name() -> String {
        "VerifyKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Ethereum address".to_string());
        schema.string().pattern = Some("^0x[0-9a-fA-F]{40}$".to_string());
        schema.into()
    }
}

impl BorshSerialize for Address {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let bytes: [u8; 20] = self.0.into();
        BorshSerialize::serialize(&bytes, writer)
    }
}

impl BorshDeserialize for Address {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let bytes = <[u8; 20] as BorshDeserialize>::deserialize_reader(reader)?;
        Ok(Self(AlloyAddress::from(bytes)))
    }
}

impl BorshSchema for Address {
    fn add_definitions_recursively(
        definitions: &mut BTreeMap<borsh::schema::Declaration, borsh::schema::Definition>,
    ) {
        <[u8; 20] as BorshSchema>::add_definitions_recursively(definitions);
    }

    fn declaration() -> borsh::schema::Declaration {
        "VerifyKey".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        let key = Address(AlloyAddress::from_slice(&[0xff; 20]));

        eprintln!("{}", near_sdk::serde_json::to_string(&key).unwrap());
    }
}
