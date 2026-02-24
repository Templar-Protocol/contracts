use near_sdk::{
    borsh::{self, io},
    json_types::U64,
};
use primitive_types::U256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct SerializableU256(
    #[borsh(serialize_with = "u256_borsh_ser", deserialize_with = "u256_borsh_de")]
    #[serde(with = "u256_serde")]
    pub primitive_types::U256,
);

impl std::ops::Deref for SerializableU256 {
    type Target = primitive_types::U256;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<primitive_types::U256> for SerializableU256 {
    fn from(value: primitive_types::U256) -> Self {
        Self(value)
    }
}

impl From<SerializableU256> for primitive_types::U256 {
    fn from(value: SerializableU256) -> Self {
        value.0
    }
}

mod u256_serde {
    use near_sdk::serde;
    use primitive_types::U256;

    pub fn serialize<S: serde::Serializer>(x: &U256, ser: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&x.to_string(), ser)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(de: D) -> Result<U256, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(de)?;
        U256::from_dec_str(&s).map_err(serde::de::Error::custom)
    }
}

fn u256_borsh_ser<W: io::Write>(x: &U256, writer: &mut W) -> Result<(), io::Error> {
    borsh::BorshSerialize::serialize(&x.0, writer)
}

fn u256_borsh_de<R: io::Read>(reader: &mut R) -> Result<U256, io::Error> {
    Ok(U256(borsh::BorshDeserialize::deserialize_reader(reader)?))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct FeedData {
    pub price: SerializableU256,
    pub package_timestamp: U64,
    pub write_timestamp: U64,
}

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;

    use super::*;

    #[test]
    fn json() {
        let fd = FeedData {
            price: SerializableU256(3333u128.into()),
            package_timestamp: U64(5555),
            write_timestamp: U64(6666),
        };

        let serialized = serde_json::to_string(&fd).unwrap();

        eprintln!("{serialized}");

        assert_eq!(
            serialized,
            r#"{"price":"3333","package_timestamp":"5555","write_timestamp":"6666"}"#,
        );

        let deserialized: FeedData = serde_json::from_str(&serialized).unwrap();

        assert_eq!(fd, deserialized);
    }
}
