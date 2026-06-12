#[cfg(feature = "borsh")]
use alloc::string::ToString;

#[cfg_attr(
    feature = "borsh",
    derive(
        ::borsh::BorshSerialize,
        ::borsh::BorshDeserialize,
        ::borsh::BorshSchema
    )
)]
#[cfg_attr(feature = "serde", derive(::serde::Serialize, ::serde::Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(
    #[cfg_attr(
        feature = "serde",
        serde(
            serialize_with = "hex::serde::serialize",
            deserialize_with = "hex::serde::deserialize"
        )
    )]
    pub [u8; 64],
);

impl AccountId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for AccountId {
    fn schema_name() -> alloc::string::String {
        alloc::string::String::from("AccountId")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = schemars::schema::SchemaObject::default();
        schema.instance_type = Some(schemars::schema::InstanceType::String.into());
        schema.metadata().description = Some(alloc::string::String::from("Account ID"));
        schema.string().pattern = Some(alloc::string::String::from("^[0-9a-fA-F]{128}$"));
        schema.string().min_length = Some(128);
        schema.string().max_length = Some(128);
        schema.into()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "schemars")]
    use super::AccountId;

    #[cfg(feature = "schemars")]
    #[test]
    fn account_id_schema_is_fixed_length_hex() {
        let schema = schemars::schema_for!(AccountId).schema;
        let string = schema
            .string
            .expect("AccountId should use string validation");

        assert_eq!(string.pattern.as_deref(), Some("^[0-9a-fA-F]{128}$"));
        assert_eq!(string.min_length, Some(128));
        assert_eq!(string.max_length, Some(128));
    }
}
