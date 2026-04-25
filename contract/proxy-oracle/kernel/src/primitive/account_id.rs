use crate::*;

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

impl schemars::JsonSchema for AccountId {
    fn schema_name() -> String {
        "AccountId".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = generator.subschema_for::<String>().into_object();
        schema.metadata().description = Some("Account ID".to_string());
        schema.into()
    }
}
