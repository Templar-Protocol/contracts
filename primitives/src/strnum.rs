mod serializable_u256;
pub use serializable_u256::SerializableU256 as SU256;

#[cfg(any(feature = "borsh", feature = "schemars"))]
use alloc::format;
#[cfg(feature = "schemars")]
use alloc::string::String;
#[cfg(any(feature = "serde", feature = "schemars"))]
use alloc::string::ToString;
use core::ops::Deref;

pub type SU64 = StrNum<u64>;
pub type SU128 = StrNum<u128>;
pub type SI64 = StrNum<i64>;
pub type SI128 = StrNum<i128>;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize, borsh::BorshSchema)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct StrNum<T>(
    #[cfg_attr(
        feature = "serde",
        serde(
            bound = "T: ToString + core::str::FromStr, <T as core::str::FromStr>::Err: core::fmt::Display",
            serialize_with = "ser_de::serialize",
            deserialize_with = "ser_de::deserialize"
        )
    )]
    pub T,
);

impl<T> StrNum<T> {
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for StrNum<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> AsRef<T> for StrNum<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for StrNum<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

#[cfg(feature = "schemars")]
impl<T> schemars::JsonSchema for StrNum<T>
where
    T: schemars::JsonSchema + ToString + core::str::FromStr,
{
    fn schema_name() -> String {
        format!("StrNum_{}", T::schema_name())
    }

    fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = schemars::schema::SchemaObject::default();
        schema.instance_type = Some(schemars::schema::InstanceType::String.into());
        schema.metadata().description = Some(format!(
            "string-serialized wrapper around {}",
            T::schema_name()
        ));
        schema.into()
    }
}

#[cfg(feature = "serde")]
mod ser_de {
    use alloc::string::{String, ToString};
    use core::str::FromStr;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: ToString>(t: &T, ser: S) -> Result<S::Ok, S::Error> {
        String::serialize(&t.to_string(), ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: FromStr>(d: D) -> Result<T, D::Error>
    where
        <T as FromStr>::Err: core::fmt::Display,
    {
        let s = String::deserialize(d)?;
        T::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "near")]
mod near {
    macro_rules! impl_from_near {
        ($n: ident, $p: ident) => {
            impl From<near_sdk::json_types::$n> for super::StrNum<$p> {
                fn from(value: near_sdk::json_types::$n) -> Self {
                    Self(value.0)
                }
            }

            impl From<super::StrNum<$p>> for near_sdk::json_types::$n {
                fn from(value: super::StrNum<$p>) -> Self {
                    Self(value.0)
                }
            }
        };
    }

    impl_from_near!(U64, u64);
    impl_from_near!(U128, u128);
    impl_from_near!(I64, i64);
    impl_from_near!(I128, i128);
}

#[cfg(test)]
#[allow(clippy::unreadable_literal)]
mod tests {
    use super::{SI128, SI64, SU128, SU64};

    #[cfg(feature = "serde")]
    #[rstest::rstest]
    #[case(SU64::from(42_u64), "\"42\"")]
    #[case(
        SU128::from(340282366920938463463374607431768211455_u128),
        "\"340282366920938463463374607431768211455\""
    )]
    #[case(SI64::from(-42_i64), "\"-42\"")]
    #[case(SI128::from(-170141183460469231731687303715884105728_i128), "\"-170141183460469231731687303715884105728\"")]
    fn serde_round_trip_string_numbers<T>(#[case] value: T, #[case] expected_json: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + core::fmt::Debug + PartialEq,
    {
        let serialized = serde_json::to_string(&value).unwrap();
        assert_eq!(serialized, expected_json);

        let deserialized: T = serde_json::from_str(expected_json).unwrap();
        assert_eq!(deserialized, value);
    }
}
