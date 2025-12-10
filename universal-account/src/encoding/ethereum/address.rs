use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::primitives::Address as AlloyAddress;
use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{borsh, near};
use schemars::JsonSchema;

#[derive(
    Clone,
    Debug,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[serde(crate = "near_sdk::serde")]
#[near(serializers = [])]
pub struct Address(pub AlloyAddress);

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Address {
    type Err = alloy::hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
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

impl JsonSchema for Address {
    fn schema_name() -> String {
        "Address".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Ethereum address".to_string());
        schema.string().pattern = Some("^0x[0-9a-fA-F]{40}$".to_string());
        schema.into()
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
    fn json_serialization() {
        let key = Address(AlloyAddress::from_slice(&[0xff; 20]));

        eprintln!("{}", near_sdk::serde_json::to_string(&key).unwrap());
    }

    #[test]
    fn to_from_str() {
        let address = Address::from_str("0xa2E641CcbEB84c6Ed1e1E43e18B720F6D5C5173E").unwrap();
        assert_eq!(
            address.to_string(),
            "0xa2E641CcbEB84c6Ed1e1E43e18B720F6D5C5173E",
        );
    }

    #[test]
    fn borsh() {
        let address = Address::from_str("0xa2E641CcbEB84c6Ed1e1E43e18B720F6D5C5173E").unwrap();
        let borsh_bytes = near_sdk::borsh::to_vec(&address).unwrap();
        let decoded: Address = near_sdk::borsh::from_slice(&borsh_bytes).unwrap();

        assert_eq!(decoded, address);
    }
}
