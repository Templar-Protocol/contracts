#[cfg(any(feature = "serde", feature = "schemars"))]
use alloc::string::String;
#[cfg(any(feature = "borsh", feature = "serde", feature = "schemars"))]
use alloc::string::ToString;
use primitive_types::U256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize, borsh::BorshSchema)
)]
pub struct SerializableU256([u64; 4]);

impl SerializableU256 {
    pub fn to_u256(self) -> U256 {
        self.into()
    }
}

impl From<U256> for SerializableU256 {
    fn from(value: U256) -> Self {
        Self(value.0)
    }
}

impl From<SerializableU256> for U256 {
    fn from(value: SerializableU256) -> Self {
        U256(value.0)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for SerializableU256 {
    fn schema_name() -> String {
        "SerializableU256".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("unsigned 256-bit integer".to_string());
        schema.into()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for SerializableU256 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&U256(self.0).to_string(), serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SerializableU256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        U256::from_dec_str(&s)
            .map(Self::from)
            .map_err(serde::de::Error::custom)
    }
}
