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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct Number(pub U256);

#[cfg(feature = "serde")]
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

            impl<'de> de::Visitor<'de> for NumberVisitor {
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

#[cfg(feature = "borsh")]
mod borsh_impl {
    use super::*;
    use alloc::collections::BTreeMap;
    use borsh::schema::{add_definition, Declaration, Definition};
    use borsh::{self, BorshDeserialize, BorshSchema, BorshSerialize};

    impl BorshSerialize for Number {
        fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
            // Serialize as 32 bytes (little-endian)
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
    use schemars::gen::SchemaGenerator;
    use schemars::schema::Schema;
    use schemars::JsonSchema;

    impl JsonSchema for Number {
        fn schema_name() -> alloc::string::String {
            "Number".to_string()
        }

        fn json_schema(generator: &mut SchemaGenerator) -> Schema {
            let mut g = generator.subschema_for::<[u8; 32]>().into_object();
            g.metadata().description = Some("256-bit Unsigned Integer".to_string());
            g.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
            g.into()
        }
    }
}

impl Number {
    /// Zero constant.
    pub const ZERO: Self = Number(U256::zero());
    /// One constant.
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
        let mut b64 = [0u8; 64];
        q.write_as_little_endian(&mut b64);
        U256::from_little_endian(&b64[..32])
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

    #[inline]
    #[must_use]
    pub fn mul_div_floor(x: Number, y: Number, denom: Number) -> Number {
        if denom.is_zero() {
            return Number::zero();
        }
        let prod = x.0.full_mul(y.0);
        let q = prod / U512::from(denom.0);
        Number(Self::as_u256_trunc(q))
    }

    #[allow(clippy::many_single_char_names)]
    #[inline]
    #[must_use]
    pub fn mul_div_ceil(x: Number, y: Number, denom: Number) -> Number {
        if denom.is_zero() {
            return Number::zero();
        }
        let prod = x.0.full_mul(y.0);
        let d = U512::from(denom.0);
        let q = prod / d;
        let r = prod % d;
        let base = Number(Self::as_u256_trunc(q));
        if r.is_zero() {
            base
        } else {
            base.saturating_add(Number::one())
        }
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
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn expected_floor(x: u128, y: u128, denom: u128) -> U256 {
        let prod = U512::from(x) * U512::from(y);
        let q = prod / U512::from(denom);
        Number::as_u256_trunc(q)
    }

    fn expected_ceil(x: u128, y: u128, denom: u128) -> U256 {
        let prod = U512::from(x) * U512::from(y);
        let d = U512::from(denom);
        let q = prod / d;
        let r = prod % d;
        let q = if r.is_zero() { q } else { q + U512::from(1u8) };
        Number::as_u256_trunc(q)
    }

    proptest! {
        #[test]
        fn mul_div_floor_matches_u512(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            let expected = expected_floor(x, y, denom);
            prop_assert_eq!(floor.0, expected);
        }

        #[test]
        fn mul_div_ceil_matches_u512(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            let expected = expected_ceil(x, y, denom);
            prop_assert_eq!(ceil.0, expected);
        }

        #[test]
        fn mul_div_zero_denom_is_zero(x in any::<u128>(), y in any::<u128>()) {
            let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(0u128));
            let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(0u128));
            prop_assert!(floor.is_zero());
            prop_assert!(ceil.is_zero());
        }

        // ===================================================================
        // Property: mul_div_floor <= mul_div_ceil (floor never exceeds ceil)
        // Invariant: For all x, y, denom > 0: floor(x*y/d) <= ceil(x*y/d)
        // ===================================================================
        #[test]
        fn mul_div_floor_leq_ceil(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            prop_assert!(floor.0 <= ceil.0, "floor {} > ceil {}", floor.0, ceil.0);
        }

        // ===================================================================
        // Property: ceil - floor <= 1 (difference is at most 1)
        // Invariant: ceil(x*y/d) - floor(x*y/d) <= 1
        // ===================================================================
        #[test]
        fn mul_div_ceil_floor_diff_at_most_one(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            let diff = ceil.0.saturating_sub(floor.0);
            prop_assert!(diff <= U256::one(), "diff {} > 1", diff);
        }

        // ===================================================================
        // Property: mul_div_floor commutativity in x and y
        // Invariant: floor(x*y/d) == floor(y*x/d)
        // ===================================================================
        #[test]
        fn mul_div_floor_commutative(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let result1 = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            let result2 = Number::mul_div_floor(Number::from(y), Number::from(x), Number::from(denom));
            prop_assert_eq!(result1.0, result2.0);
        }

        // ===================================================================
        // Property: mul_div_ceil commutativity in x and y
        // Invariant: ceil(x*y/d) == ceil(y*x/d)
        // ===================================================================
        #[test]
        fn mul_div_ceil_commutative(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let result1 = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            let result2 = Number::mul_div_ceil(Number::from(y), Number::from(x), Number::from(denom));
            prop_assert_eq!(result1.0, result2.0);
        }

        // ===================================================================
        // Property: Identity - mul_div with denom=1 equals x*y
        // Invariant: floor(x*y/1) == x*y (when fits in U256)
        // ===================================================================
        #[test]
        fn mul_div_floor_identity_denom_one(
            x in 0u128..=u64::MAX as u128,
            y in 0u128..=u64::MAX as u128,
        ) {
            let result = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(1u128));
            let expected = U256::from(x) * U256::from(y);
            prop_assert_eq!(result.0, expected);
        }

        // ===================================================================
        // Property: Zero x or y produces zero
        // Invariant: floor(0*y/d) == 0 and floor(x*0/d) == 0
        // ===================================================================
        #[test]
        fn mul_div_floor_zero_factor(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let r1 = Number::mul_div_floor(Number::zero(), Number::from(y), Number::from(denom));
            let r2 = Number::mul_div_floor(Number::from(x), Number::zero(), Number::from(denom));
            prop_assert!(r1.is_zero());
            prop_assert!(r2.is_zero());
        }

        // ===================================================================
        // Property: Division by self gives x when y == denom
        // Invariant: floor(x*d/d) == x
        // ===================================================================
        #[test]
        fn mul_div_floor_self_division(
            x in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let result = Number::mul_div_floor(Number::from(x), Number::from(denom), Number::from(denom));
            prop_assert_eq!(result.0, U256::from(x));
        }

        // ===================================================================
        // Property: saturating_add doesn't overflow
        // Invariant: a.saturating_add(b) >= a (for any a, b)
        // ===================================================================
        #[test]
        fn saturating_add_no_overflow(a in any::<u128>(), b in any::<u128>()) {
            let na = Number::from(a);
            let nb = Number::from(b);
            let result = na.saturating_add(nb);
            prop_assert!(result.0 >= na.0, "saturating_add decreased value");
        }

        // ===================================================================
        // Property: saturating_sub doesn't underflow
        // Invariant: a.saturating_sub(b) <= a (for any a, b)
        // ===================================================================
        #[test]
        fn saturating_sub_no_underflow(a in any::<u128>(), b in any::<u128>()) {
            let na = Number::from(a);
            let nb = Number::from(b);
            let result = na.saturating_sub(nb);
            prop_assert!(result.0 <= na.0, "saturating_sub increased value");
        }

        // ===================================================================
        // Property: saturating_add commutativity
        // Invariant: a.saturating_add(b) == b.saturating_add(a)
        // ===================================================================
        #[test]
        fn saturating_add_commutative(a in any::<u128>(), b in any::<u128>()) {
            let na = Number::from(a);
            let nb = Number::from(b);
            let r1 = na.saturating_add(nb);
            let r2 = nb.saturating_add(na);
            prop_assert_eq!(r1.0, r2.0);
        }

        // ===================================================================
        // Property: saturating_add identity
        // Invariant: a.saturating_add(0) == a
        // ===================================================================
        #[test]
        fn saturating_add_identity(a in any::<u128>()) {
            let na = Number::from(a);
            let result = na.saturating_add(Number::zero());
            prop_assert_eq!(result.0, na.0);
        }

        // ===================================================================
        // Property: saturating_sub identity
        // Invariant: a.saturating_sub(0) == a
        // ===================================================================
        #[test]
        fn saturating_sub_identity(a in any::<u128>()) {
            let na = Number::from(a);
            let result = na.saturating_sub(Number::zero());
            prop_assert_eq!(result.0, na.0);
        }

        // ===================================================================
        // Property: saturating_sub self produces zero
        // Invariant: a.saturating_sub(a) == 0
        // ===================================================================
        #[test]
        fn saturating_sub_self_is_zero(a in any::<u128>()) {
            let na = Number::from(a);
            let result = na.saturating_sub(na);
            prop_assert!(result.is_zero());
        }

        // ===================================================================
        // Property: as_u128_trunc returns lower bits
        // Invariant: Number::from(x).as_u128_trunc() == x for x: u128
        // ===================================================================
        #[test]
        fn as_u128_trunc_roundtrip(x in any::<u128>()) {
            let n = Number::from(x);
            let back = n.as_u128_trunc();
            prop_assert_eq!(back, x);
        }

        // ===================================================================
        // Property: as_u128_saturating for small values
        // Invariant: For x <= u128::MAX, as_u128_saturating(Number::from(x)) == x
        // ===================================================================
        #[test]
        fn as_u128_saturating_small_values(x in any::<u128>()) {
            let n = Number::from(x);
            let sat = n.as_u128_saturating();
            prop_assert_eq!(sat, x);
        }

        // ===================================================================
        // Property: Monotonicity of mul_div_floor in x
        // Invariant: If x1 <= x2 then floor(x1*y/d) <= floor(x2*y/d)
        // ===================================================================
        #[test]
        fn mul_div_floor_monotonic_in_x(
            x1 in any::<u128>(),
            x2 in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let (lo, hi) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
            let r_lo = Number::mul_div_floor(Number::from(lo), Number::from(y), Number::from(denom));
            let r_hi = Number::mul_div_floor(Number::from(hi), Number::from(y), Number::from(denom));
            prop_assert!(r_lo.0 <= r_hi.0, "not monotonic: {} > {}", r_lo.0, r_hi.0);
        }

        // ===================================================================
        // Property: Monotonicity of mul_div_floor in y
        // Invariant: If y1 <= y2 then floor(x*y1/d) <= floor(x*y2/d)
        // ===================================================================
        #[test]
        fn mul_div_floor_monotonic_in_y(
            x in any::<u128>(),
            y1 in any::<u128>(),
            y2 in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let (lo, hi) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
            let r_lo = Number::mul_div_floor(Number::from(x), Number::from(lo), Number::from(denom));
            let r_hi = Number::mul_div_floor(Number::from(x), Number::from(hi), Number::from(denom));
            prop_assert!(r_lo.0 <= r_hi.0, "not monotonic: {} > {}", r_lo.0, r_hi.0);
        }

        // ===================================================================
        // Property: Anti-monotonicity of mul_div_floor in denom
        // Invariant: If d1 <= d2 then floor(x*y/d1) >= floor(x*y/d2)
        // ===================================================================
        #[test]
        fn mul_div_floor_antimonotonic_in_denom(
            x in any::<u128>(),
            y in any::<u128>(),
            d1 in 1u128..=u128::MAX,
            d2 in 1u128..=u128::MAX,
        ) {
            let (lo_d, hi_d) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
            let r_lo_d = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(lo_d));
            let r_hi_d = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(hi_d));
            // Smaller denominator => larger result
            prop_assert!(r_lo_d.0 >= r_hi_d.0, "denom monotonicity violated: {} < {}", r_lo_d.0, r_hi_d.0);
        }
    }

    // =========================================================================
    // Unit tests for basic Number operations and operators
    // =========================================================================

    #[test]
    fn number_constants() {
        assert!(Number::ZERO.is_zero());
        assert!(Number::ONE.is_one());
        assert!(Number::zero().is_zero());
        assert!(Number::one().is_one());
    }

    #[test]
    fn number_from_u128_into_u128() {
        let val: u128 = 123456789;
        let n = Number::from(val);
        let back: u128 = n.into();
        assert_eq!(back, val);
    }

    #[test]
    fn number_div_by_u128() {
        let n = Number::from(100u128);
        let result = n / 10u128;
        assert_eq!(u128::from(result), 10);
    }

    #[test]
    fn number_div_by_u256() {
        let n = Number::from(100u128);
        let result = n / U256::from(5u128);
        assert_eq!(u128::from(result), 20);
    }

    #[test]
    fn number_div_by_number() {
        let a = Number::from(100u128);
        let b = Number::from(4u128);
        let result = a / b;
        assert_eq!(u128::from(result), 25);
    }

    #[test]
    fn number_add() {
        let a = Number::from(50u128);
        let b = Number::from(30u128);
        let result = a + b;
        assert_eq!(u128::from(result), 80);
    }

    #[test]
    fn number_sub() {
        let a = Number::from(100u128);
        let b = Number::from(40u128);
        let result = a - b;
        assert_eq!(u128::from(result), 60);
    }

    #[test]
    fn number_from_into_u256() {
        let u = U256::from(999u128);
        let n: Number = u.into();
        let back: U256 = n.into();
        assert_eq!(back, u);
    }

    #[test]
    fn as_u128_saturating_large_value() {
        // Create a Number that exceeds u128::MAX
        let large = Number(U256::from(u128::MAX) + U256::from(1u128));
        assert_eq!(large.as_u128_saturating(), u128::MAX);
    }

    #[test]
    fn number_is_zero_is_one() {
        let zero = Number::from(0u128);
        let one = Number::from(1u128);
        let two = Number::from(2u128);

        assert!(zero.is_zero());
        assert!(!zero.is_one());

        assert!(!one.is_zero());
        assert!(one.is_one());

        assert!(!two.is_zero());
        assert!(!two.is_one());
    }

}
