use std::str::FromStr;

use near_sdk::{
    env, near,
    serde::{
        self,
        de::{self, DeserializeOwned},
        Deserialize, Serialize,
    },
    serde_json,
};

use crate::authentication::MagicNumber;

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct WithRawString<T> {
    pub raw: String,
    pub parsed: T,
}

impl<T> WithRawString<T> {
    /// # Panics
    ///
    /// - If serialization fails.
    pub fn from_parsed(value: T) -> Self
    where
        T: Serialize,
    {
        #[allow(clippy::unwrap_used, reason = "This method panics")]
        let raw = serde_json::to_string(&value).unwrap();
        Self { raw, parsed: value }
    }
}

impl<T: MagicNumber> WithRawString<T> {
    pub fn bytes_with_magic_number(&self) -> Vec<u8> {
        [T::MAGIC_NUMBER, self.raw.as_bytes()].concat()
    }

    pub fn hash(&self) -> [u8; 32] {
        env::sha256_array(&self.bytes_with_magic_number())
    }
}

impl<T: DeserializeOwned> FromStr for WithRawString<T> {
    type Err = serde_json::Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::try_from(raw.to_string())
    }
}

impl<T: DeserializeOwned> TryFrom<String> for WithRawString<T> {
    type Error = serde_json::Error;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        let mut d = serde_json::Deserializer::from_str(&r#raw);
        let parsed: T = Deserialize::deserialize(&mut d)?;

        Ok(Self { raw, parsed })
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for WithRawString<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw: String = Deserialize::deserialize(deserializer)?;
        Self::try_from(raw).map_err(de::Error::custom)
    }
}

impl<T> Serialize for WithRawString<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.raw, serializer)
    }
}

impl<T: schemars::JsonSchema> schemars::JsonSchema for WithRawString<T> {
    fn schema_name() -> String {
        T::schema_name()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        T::json_schema(gen)
    }
}
