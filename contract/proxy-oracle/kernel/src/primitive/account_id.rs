#[cfg(feature = "borsh")]
use crate::std::string::ToString;

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

    #[cfg(feature = "near")]
    fn from_near_account_id(account_id: &near_sdk::AccountId) -> Self {
        let mut bytes = [0u8; 64];
        let source = account_id.as_bytes();
        bytes[..source.len()].copy_from_slice(source);
        Self(bytes)
    }
}

#[cfg(feature = "near")]
impl From<near_sdk::AccountId> for AccountId {
    fn from(account_id: near_sdk::AccountId) -> Self {
        Self::from_near_account_id(&account_id)
    }
}

#[cfg(feature = "near")]
impl From<&near_sdk::AccountId> for AccountId {
    fn from(account_id: &near_sdk::AccountId) -> Self {
        Self::from_near_account_id(account_id)
    }
}

#[cfg(feature = "near")]
impl From<&near_sdk::AccountIdRef> for AccountId {
    fn from(account_id: &near_sdk::AccountIdRef) -> Self {
        let mut bytes = [0u8; 64];
        let source = account_id.as_bytes();
        bytes[..source.len()].copy_from_slice(source);
        Self(bytes)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for AccountId {
    fn schema_name() -> alloc::string::String {
        alloc::string::String::from("AccountId")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = generator
            .subschema_for::<alloc::string::String>()
            .into_object();
        schema.metadata().description = Some(alloc::string::String::from("Account ID"));
        schema.into()
    }
}

#[cfg(test)]
mod tests {
    use super::AccountId;

    #[cfg(feature = "near")]
    #[test]
    fn converts_near_account_id_to_zero_padded_bytes() {
        let account_id: near_sdk::AccountId = "oracle.near".parse().unwrap();
        let converted = AccountId::from(account_id.clone());

        assert_eq!(
            &converted.as_bytes()[..account_id.len()],
            account_id.as_bytes()
        );
        assert!(converted.as_bytes()[account_id.len()..]
            .iter()
            .all(|byte| *byte == 0));
    }
}
