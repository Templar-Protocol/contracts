use near_sdk::{
    near,
    serde::{self, de, Deserialize, Serialize},
    serde_json,
};

#[derive(Clone, Debug)]
#[near(serializers = [])]
pub struct WithRawString<T> {
    pub raw: String,
    pub parsed: T,
}

impl<'de, T: for<'a> Deserialize<'a>> Deserialize<'de> for WithRawString<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw: String = Deserialize::deserialize(deserializer)?;
        let mut d = serde_json::Deserializer::from_str(&r#raw);
        let parsed: T = Deserialize::deserialize(&mut d).map_err(de::Error::custom)?;

        Ok(Self { raw, parsed })
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
