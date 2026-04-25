mod serializable_u256;
pub use serializable_u256::SerializableU256 as SU256;

use core::ops::Deref;

#[allow(unused_imports)]
use crate::*;

pub type SU64 = StrNum<u64>;
pub type SU128 = StrNum<u128>;
pub type SI64 = StrNum<i64>;
pub type SI128 = StrNum<i128>;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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

#[cfg(feature = "serde")]
mod ser_de {
    use core::str::FromStr;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::*;

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
