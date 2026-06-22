//! Chain-agnostic Number type for precise 256-bit arithmetic.
//!
//! Provides a U256-backed wrapper for overflow-safe calculations.

use core::ops::{Add, Div, Sub};

use derive_more::{From, Into};
use primitive_types::{U256, U512};

/// Wider type for intermediate calculations.
pub type WIDE = U512;

/// A 256-bit unsigned integer wrapper for precise arithmetic.
///
/// When the `serde` feature is enabled, serializes to/from a decimal string
/// for compatibility with JSON-based APIs.
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct Number(pub U256);

#[cfg(all(feature = "serde", not(feature = "postcard")))]
mod serde_impl {
    use super::*;
    use alloc::string::ToString;
    use core::fmt;
    use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for Number {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            // Serialize as decimal string for JSON compatibility
            let s = self.0.to_string();
            serializer.serialize_str(&s)
        }
    }

    impl<'de> Deserialize<'de> for Number {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct NumberVisitor;

            impl de::Visitor<'_> for NumberVisitor {
                type Value = Number;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("a decimal string representing a U256")
                }

                fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
                where
                    E: de::Error,
                {
                    U256::from_dec_str(v)
                        .map(Number)
                        .map_err(|_| E::custom("invalid decimal string for U256"))
                }
            }

            deserializer.deserialize_str(NumberVisitor)
        }
    }
}

#[cfg(feature = "postcard")]
mod postcard_serde_impl {
    use super::*;
    #[cfg(not(feature = "soroban"))]
    use serde::de;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for Number {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            #[cfg(feature = "soroban")]
            {
                Number::as_u128_if_fits(self.0)
                    .ok_or_else(|| {
                        serde::ser::Error::custom("Number exceeds u128 for Soroban postcard")
                    })
                    .and_then(|value| serializer.serialize_u128(value))
            }

            #[cfg(not(feature = "soroban"))]
            {
                let mut bytes = [0u8; 32];
                self.0.write_as_little_endian(&mut bytes);
                serializer.serialize_bytes(&bytes)
            }
        }
    }

    impl<'de> Deserialize<'de> for Number {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[cfg(feature = "soroban")]
            {
                u128::deserialize(deserializer).map(Number::from)
            }

            #[cfg(not(feature = "soroban"))]
            {
                struct NumberVisitor;

                impl<'de> de::Visitor<'de> for NumberVisitor {
                    type Value = Number;

                    fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                        formatter.write_str("exactly 32 bytes for little-endian U256")
                    }

                    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        if v.len() != 32 {
                            return Err(E::custom("expected exactly 32 bytes for U256"));
                        }
                        Ok(Number(U256::from_little_endian(v)))
                    }
                }

                deserializer.deserialize_bytes(NumberVisitor)
            }
        }
    }
}

#[cfg(feature = "borsh")]
mod borsh_impl {
    use super::*;
    use borsh::{self, BorshDeserialize, BorshSerialize};

    impl BorshSerialize for Number {
        fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
            let mut bytes = [0u8; 32];
            self.0.write_as_little_endian(&mut bytes);
            writer.write_all(&bytes)
        }
    }

    impl BorshDeserialize for Number {
        fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
            let mut bytes = [0u8; 32];
            reader.read_exact(&mut bytes)?;
            Ok(Number(U256::from_little_endian(&bytes)))
        }
    }
}

#[cfg(feature = "borsh-schema")]
mod borsh_schema_impl {
    use super::*;
    use alloc::collections::BTreeMap;
    use borsh::schema::{add_definition, Declaration, Definition};
    use borsh::BorshSchema;

    impl BorshSchema for Number {
        fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
            let definition = Definition::Primitive(32);
            add_definition(Self::declaration(), definition, definitions);
        }

        fn declaration() -> Declaration {
            "Number".into()
        }
    }
}

#[cfg(feature = "schemars")]
mod schemars_impl {
    use super::*;
    use alloc::string::ToString;
    use schemars::r#gen::SchemaGenerator;
    use schemars::schema::Schema;
    use schemars::JsonSchema;

    impl JsonSchema for Number {
        fn schema_name() -> alloc::string::String {
            "Number".to_string()
        }

        fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
            let mut g = schemars::schema::SchemaObject::default();
            g.metadata().description = Some("256-bit Unsigned Integer".to_string());
            g.instance_type = Some(schemars::schema::InstanceType::String.into());
            g.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
            g.into()
        }
    }
}

impl Number {
    pub const ZERO: Self = Number(U256::zero());
    pub const ONE: Self = Number(U256::one());

    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }

    #[inline]
    #[must_use]
    pub const fn one() -> Self {
        Self::ONE
    }

    #[inline]
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    #[inline]
    #[must_use]
    pub fn is_one(&self) -> bool {
        self.0 == U256::one()
    }

    #[inline]
    #[must_use]
    pub fn as_u128_trunc(self) -> u128 {
        let mut b32 = [0u8; 32];
        self.0.write_as_little_endian(&mut b32);
        let mut b16 = [0u8; 16];
        b16.copy_from_slice(&b32[..16]);
        u128::from_le_bytes(b16)
    }

    #[inline]
    #[must_use]
    pub fn as_u128_saturating(self) -> u128 {
        if self.0 .0[2] != 0 || self.0 .0[3] != 0 {
            u128::MAX
        } else {
            self.0.as_u128()
        }
    }

    #[inline]
    pub(crate) fn as_u256_trunc(q: U512) -> U256 {
        let U512(ref limbs) = q;
        U256([limbs[0], limbs[1], limbs[2], limbs[3]])
    }

    #[inline]
    pub(crate) fn as_u128_if_fits(value: U256) -> Option<u128> {
        let U256(ref limbs) = value;
        if limbs[2] != 0 || limbs[3] != 0 {
            return None;
        }
        Some((u128::from(limbs[1]) << 64) | u128::from(limbs[0]))
    }

    #[inline]
    #[must_use]
    pub fn saturating_add(self, other: Number) -> Number {
        Number(self.0.saturating_add(other.0))
    }

    #[inline]
    #[must_use]
    pub fn saturating_sub(self, other: Number) -> Number {
        Number(self.0.saturating_sub(other.0))
    }

    #[inline(never)]
    fn mul_div_with_rounding(x: Number, y: Number, denom: Number, round_up: bool) -> Number {
        // Fast path: zero inputs
        if x.is_zero() || y.is_zero() {
            return Number::zero();
        }
        if denom.is_zero() {
            return Number::zero();
        }
        // Fast path: denom == 1 (identity division)
        if denom.is_one() {
            return Number(x.0.saturating_mul(y.0));
        }
        // Fast path: cancellation when one factor equals denom
        if x.0 == denom.0 {
            return y;
        }
        if y.0 == denom.0 {
            return x;
        }
        if let (Some(x128), Some(y128), Some(denom128)) = (
            Self::as_u128_if_fits(x.0),
            Self::as_u128_if_fits(y.0),
            Self::as_u128_if_fits(denom.0),
        ) {
            if let Some(prod) = x128.checked_mul(y128) {
                let q = prod / denom128;
                if !round_up {
                    return Number::from(q);
                }
                let r = prod % denom128;
                return if r == 0 {
                    Number::from(q)
                } else {
                    Number::from(q.saturating_add(1))
                };
            }
        }
        // General path: use U512 for overflow-safe multiplication
        let prod = x.0.full_mul(y.0);
        let d = U512::from(denom.0);
        let q = prod / d;
        let base = Number(Self::as_u256_trunc(q));
        if !round_up {
            return base;
        }
        let r = prod % d;
        if r.is_zero() {
            base
        } else {
            base.saturating_add(Number::one())
        }
    }

    #[inline]
    #[must_use]
    pub fn mul_div_floor(x: Number, y: Number, denom: Number) -> Number {
        Self::mul_div_with_rounding(x, y, denom, false)
    }

    #[inline]
    #[must_use]
    pub fn mul_div_ceil(x: Number, y: Number, denom: Number) -> Number {
        Self::mul_div_with_rounding(x, y, denom, true)
    }
}

impl From<u128> for Number {
    #[inline]
    fn from(v: u128) -> Self {
        Number(U256::from(v))
    }
}
impl From<Number> for u128 {
    #[inline]
    fn from(n: Number) -> u128 {
        n.as_u128_trunc()
    }
}
impl Div<u128> for Number {
    type Output = Number;
    #[inline]
    fn div(self, rhs: u128) -> Number {
        Number(self.0 / U256::from(rhs))
    }
}
impl Div<U256> for Number {
    type Output = Number;
    #[inline]
    fn div(self, rhs: U256) -> Number {
        Number(self.0 / rhs)
    }
}
impl Div<Number> for Number {
    type Output = Number;
    #[inline]
    fn div(self, rhs: Number) -> Number {
        Number(self.0 / rhs.0)
    }
}
impl Add<Number> for Number {
    type Output = Number;
    #[inline]
    fn add(self, rhs: Number) -> Number {
        Number(self.0 + rhs.0)
    }
}
impl Sub<Number> for Number {
    type Output = Number;
    #[inline]
    fn sub(self, rhs: Number) -> Number {
        Number(self.0 - rhs.0)
    }
}

#[cfg(test)]
mod tests;
