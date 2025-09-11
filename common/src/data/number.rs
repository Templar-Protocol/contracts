use std::{
    fmt::{Debug, Display},
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
    str::FromStr,
};

use near_sdk::{
    borsh::{BorshDeserialize, BorshSchema, BorshSerialize},
    serde::{self, Deserialize, Serialize},
};
use primitive_types::U512;
use schemars::JsonSchema;

#[cfg(test)]
mod tests;

pub const FRACTIONAL_BITS: usize = 128;
/// `floor(FRACTIONAL_BITS / log2(10))`
pub const FRACTIONAL_DECIMAL_DIGITS: usize = 38;
/// `floor((512 - FRACTIONAL_BITS) / log2(10))`
pub const WHOLE_DECIMAL_DIGITS: usize = 115;

/// Because `U512::exp10` is recursive, linear-time, and prone to stack overflows.
fn u512_pow10(mut exponent: u32) -> U512 {
    let mut y = U512::one();
    let mut x = U512::from(10);

    while exponent > 1 {
        if exponent % 2 == 1 {
            y *= x;
        }
        x *= x;
        exponent >>= 1;
    }

    x * y
}

#[macro_export]
macro_rules! dec {
    ($s:literal) => {
        <$crate::number::Decimal as std::str::FromStr>::from_str($s).unwrap()
    };
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal {
    repr: U512,
}

impl Default for Decimal {
    fn default() -> Self {
        Self::ZERO
    }
}

impl JsonSchema for Decimal {
    fn schema_name() -> String {
        "Decimal".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("512-bit fixed-precision decimal".to_string());
        schema.string().pattern = Some("^(0|[1-9][0-9]{0,115})(\\.[0-9]{1,38})?$".to_string());
        schema.into()
    }
}

impl BorshSchema for Decimal {
    fn add_definitions_recursively(
        definitions: &mut std::collections::BTreeMap<
            near_sdk::borsh::schema::Declaration,
            near_sdk::borsh::schema::Definition,
        >,
    ) {
        <[u64; 8] as BorshSchema>::add_definitions_recursively(definitions);
    }

    fn declaration() -> near_sdk::borsh::schema::Declaration {
        String::from("Decimal")
    }
}

impl BorshSerialize for Decimal {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        BorshSerialize::serialize(&self.repr.0, writer)
    }
}

impl BorshDeserialize for Decimal {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Ok(Self {
            repr: U512(BorshDeserialize::deserialize_reader(reader)?),
        })
    }
}

impl Serialize for Decimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        serializer.serialize_str(&self.to_fixed(FRACTIONAL_DECIMAL_DIGITS))
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Decimal::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Decimal {
    /// When converting to and from strings, we do not guarantee accurate
    /// representation of bits lower than this.
    const REPR_EPSILON: U512 = U512([0b1000, 0, 0, 0, 0, 0, 0, 0]);

    pub const MAX: Self = Self { repr: U512::MAX };
    pub const MIN: Self = Self { repr: U512::zero() };

    pub const ZERO: Self = Self { repr: U512::zero() };
    pub const ONE_HALF: Self = Self {
        repr: U512([0, 0x8000_0000_0000_0000, 0, 0, 0, 0, 0, 0]),
    };
    #[rustfmt::skip]
    pub const LN2: Self = Self {
        repr: U512([0xC9E3_B398_03F2_F6B0, 0xB172_17F7_D1CF_79AB, 0, 0, 0, 0, 0, 0]),
    };
    pub const ONE: Self = Self {
        repr: U512([0, 0, 1, 0, 0, 0, 0, 0]),
    };
    pub const TWO: Self = Self {
        repr: U512([0, 0, 2, 0, 0, 0, 0, 0]),
    };
    #[rustfmt::skip]
    pub const E: Self = Self {
        repr: U512([0xBF71_5880_9CF4_F3C9, 0xB7E1_5162_8AED_2A6A, 2, 0, 0, 0, 0, 0]),
    };

    pub fn as_repr(self) -> [u64; 8] {
        self.repr.0
    }

    pub fn is_zero(&self) -> bool {
        self.repr.is_zero()
    }

    pub fn near_equal(self, other: Self) -> bool {
        self.abs_diff(other).repr <= Self::REPR_EPSILON
    }

    #[must_use]
    pub fn pow(self, mut exponent: i32) -> Self {
        if exponent == 0 {
            return Self::ONE;
        }

        let exponent_is_negative = if exponent < 0 {
            exponent = -exponent;
            true
        } else {
            false
        };

        let mut y = Self::ONE;
        let mut x = self;

        while exponent > 1 {
            if exponent % 2 == 1 {
                y *= x;
            }
            x *= x;
            exponent >>= 1;
        }

        let result = x * y;

        if exponent_is_negative {
            Decimal::ONE / result
        } else {
            result
        }
    }

    /// Calculates `2^exponent`.
    pub fn pow2_int(exponent: u32) -> Option<Self> {
        #[allow(clippy::cast_possible_truncation)]
        if exponent > 512 - FRACTIONAL_BITS as u32 {
            None
        } else {
            Some(Self {
                repr: Self::ONE.repr << exponent,
            })
        }
    }

    fn pow2_frac(self) -> Self {
        const MAX_ITERATIONS: u32 = 35; // n=35 is smallest n where n! >= 2^128
        debug_assert!(self <= Self::ONE);

        let mut sum = Self::ONE;
        let mut term = Self::ONE;
        let numerator = self * Self::LN2;

        for n in 1..=MAX_ITERATIONS {
            term *= numerator / n;
            if term == Self::ZERO {
                break;
            }
            sum += &term;
        }

        sum
    }

    pub fn pow2(self) -> Option<Self> {
        let whole = u32::try_from(self.to_u128_floor()?).ok()?;
        let frac = self - whole;

        Some(Self::pow2_int(whole)? * Self::pow2_frac(frac))
    }

    #[must_use]
    pub fn mul_pow10(self, exponent: i32) -> Option<Self> {
        if exponent == 0 || self.is_zero() {
            return Some(self);
        }

        let abs_exponent = exponent.abs_diff(0);
        if (abs_exponent as usize) > WHOLE_DECIMAL_DIGITS + FRACTIONAL_DECIMAL_DIGITS {
            return None;
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        if exponent >= 0 {
            let operand = u512_pow10(abs_exponent);
            let shift_bits = u32::try_from(operand.bits()).ok()?;
            // Avoid overflow.
            if self.repr.leading_zeros() >= shift_bits {
                let repr = self.repr * operand;
                return Some(Self { repr });
            }
        } else {
            let operand = u512_pow10(exponent.abs_diff(0));
            let shift_bits = u32::try_from(operand.bits()).ok()?.checked_add(1)?;
            // Do allow precision loss, but don't allow going all the way to zero.
            if self.repr.leading_zeros().checked_add(shift_bits)? < 512 {
                let repr = self.repr / operand;
                return Some(Self { repr });
            }
        }

        None
    }

    #[must_use]
    pub fn abs_diff(self, other: Self) -> Self {
        if self > other {
            self - other
        } else {
            other - self
        }
    }

    pub fn to_u128_floor(self) -> Option<u128> {
        let truncated = self.repr >> FRACTIONAL_BITS;
        if truncated.bits() <= 128 {
            Some(truncated.as_u128())
        } else {
            None
        }
    }

    pub fn to_u128_ceil(self) -> Option<u128> {
        let truncated = self.repr >> FRACTIONAL_BITS;
        if truncated.bits() <= 128 {
            if self.fractional_part().is_zero() {
                Some(truncated.as_u128())
            } else {
                truncated.as_u128().checked_add(1)
            }
        } else {
            None
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Lossiness is acceptable for this function"
    )]
    pub fn to_f64_lossy(self) -> f64 {
        let frac = self.repr.low_u128() as f64 / 2f64.powi(FRACTIONAL_BITS as i32);
        let low = (self.repr >> FRACTIONAL_BITS).low_u128() as f64;
        let high = (self.repr >> (FRACTIONAL_BITS * 2)).low_u128() as f64 * 2f64.powi(128);

        high + low + frac
    }

    pub fn to_fixed(&self, precision: usize) -> String {
        let precision = precision.min(FRACTIONAL_DECIMAL_DIGITS);
        let (fractional_part, overflow) = self.fractional_part_to_dec_string(precision, false);
        let fractional_part_trimmed = fractional_part.trim_end_matches('0');
        let repr = if overflow {
            self.repr.saturating_add(Self::ONE.repr)
        } else {
            self.repr
        };
        if fractional_part_trimmed.is_empty() {
            format!("{}", repr >> FRACTIONAL_BITS)
        } else {
            format!("{}.{fractional_part_trimmed}", repr >> FRACTIONAL_BITS)
        }
    }

    fn fractional_part(&self) -> U512 {
        U512([self.repr.0[0], self.repr.0[1], 0, 0, 0, 0, 0, 0])
    }

    pub fn fractional_part_as_u128_dividend(&self) -> u128 {
        u128::from(self.repr.0[0]) | (u128::from(self.repr.0[1]) << 64)
    }

    fn epsilon_round(repr: U512) -> U512 {
        (repr + (Self::REPR_EPSILON >> 1)) & !(Self::REPR_EPSILON - 1)
    }

    fn fractional_part_to_dec_string(&self, precision: usize, round_up: bool) -> (String, bool) {
        let mut s = Vec::with_capacity(precision);
        let mut f = self.fractional_part();
        let mut overflow = false;

        if round_up {
            let plus_two = f.saturating_add(2.into());
            overflow = plus_two.0[2] != 0;
            f = U512([plus_two.0[0], plus_two.0[1], 0, 0, 0, 0, 0, 0]);
        }

        for _ in 0..precision {
            if f.is_zero() {
                break;
            }

            f *= 10;

            let digit = (f / Self::ONE.repr).low_u64();
            #[allow(clippy::cast_possible_truncation)]
            s.push(digit as u8 + b'0');

            f %= Self::ONE.repr;
        }

        if !round_up && !f.is_zero() && (U512::MAX - 2 >= self.repr) {
            return self.fractional_part_to_dec_string(precision, true);
        }

        // Safety: all digits are guaranteed to be in range 0x30..=0x39
        (unsafe { String::from_utf8_unchecked(s) }, overflow)
    }
}

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("Failed to parse decimal")]
    pub struct DecimalParseError;
}

impl FromStr for Decimal {
    type Err = error::DecimalParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (whole, frac) = if let Some((whole, frac)) = s.split_once('.') {
            (whole, Some(frac))
        } else {
            (s, None)
        };

        let whole =
            U512::from_dec_str(whole).map_err(|_| error::DecimalParseError)? << FRACTIONAL_BITS;

        if let Some(frac) = frac {
            let mut f = U512::zero();
            let mut div = 10u128;

            for c in frac.chars().take(FRACTIONAL_DECIMAL_DIGITS) {
                if let Some(d) = c.to_digit(10) {
                    if d != 0 {
                        let d = (U512::from(d) << (FRACTIONAL_BITS * 2)) / div;
                        f += d;
                    }
                    if let Some(next_div) = div.checked_mul(10) {
                        div = next_div;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            Ok(Self {
                repr: whole.saturating_add(Decimal::epsilon_round(f >> FRACTIONAL_BITS)),
            })
        } else {
            Ok(Self { repr: whole })
        }
    }
}

impl Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_f64_lossy())
    }
}

impl Debug for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_fixed(FRACTIONAL_DECIMAL_DIGITS))
    }
}

macro_rules! impl_self {
    ($s:ty,$t:ty) => {
        impl Add<$t> for $s {
            type Output = Decimal;

            fn add(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: self.repr.add(rhs.repr),
                }
            }
        }

        impl Sub<$t> for $s {
            type Output = Decimal;

            fn sub(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: self.repr.sub(rhs.repr),
                }
            }
        }

        impl Mul<$t> for $s {
            type Output = Decimal;

            fn mul(self, rhs: $t) -> Self::Output {
                #[allow(clippy::cast_possible_truncation)]
                let mut shr = FRACTIONAL_BITS as u32;
                let shr_self = self.repr.trailing_zeros().min(shr);
                let self_repr = self.repr >> shr_self;
                shr -= shr_self;
                let shr_rhs = rhs.repr.trailing_zeros().min(shr);
                let rhs_repr = rhs.repr >> shr_rhs;
                shr -= shr_rhs;
                Decimal {
                    repr: (self_repr * rhs_repr) >> shr,
                }
            }
        }

        impl Div<$t> for $s {
            type Output = Decimal;

            fn div(self, rhs: $t) -> Self::Output {
                #[allow(clippy::cast_possible_truncation)]
                let mut sh = FRACTIONAL_BITS as u32;
                let sh_self = self.repr.leading_zeros().min(sh);
                let self_repr = self.repr << sh_self;
                sh -= sh_self;
                let sh_rhs = rhs.repr.trailing_zeros().min(sh);
                let rhs_repr = rhs.repr >> sh_rhs;
                sh -= sh_rhs;
                Decimal {
                    repr: (self_repr / rhs_repr) << sh,
                }
            }
        }
    };
}

impl_self!(Decimal, Decimal);
impl_self!(&Decimal, Decimal);
impl_self!(Decimal, &Decimal);
impl_self!(&Decimal, &Decimal);

macro_rules! impl_self_assign {
    ($s:ty,$t:ty) => {
        impl AddAssign<$t> for $s {
            fn add_assign(&mut self, rhs: $t) {
                self.repr += rhs.repr;
            }
        }

        impl SubAssign<$t> for $s {
            fn sub_assign(&mut self, rhs: $t) {
                self.repr -= rhs.repr;
            }
        }

        impl DivAssign<$t> for $s {
            fn div_assign(&mut self, rhs: $t) {
                self.repr = (*self / rhs).repr;
            }
        }

        impl MulAssign<$t> for $s {
            fn mul_assign(&mut self, rhs: $t) {
                self.repr = (*self * rhs).repr;
            }
        }
    };
}

impl_self_assign!(Decimal, Decimal);
impl_self_assign!(Decimal, &Decimal);

macro_rules! impl_int {
    ($t:ty) => {
        impl_int!(@from $t);
        impl_int!(@ops $t, Decimal);
        impl_int!(@ops $t, &Decimal);
    };

    (@from $t:ty) => {
        impl From<$t> for Decimal {
            fn from(value: $t) -> Self {
                Self {
                    repr: U512::from(value) << FRACTIONAL_BITS,
                }
            }
        }
    };

    (@ops $t:ty,$s:ty) => {
        impl Mul<$t> for $s {
            type Output = Decimal;

            fn mul(self, rhs: $t) -> Self::Output {
                Decimal { repr: self.repr * U512::from(rhs) }
            }
        }

        impl Mul<$s> for $t {
            type Output = Decimal;

            fn mul(self, rhs: $s) -> Self::Output {
                Decimal { repr: U512::from(self) * rhs.repr }
            }
        }

        impl Div<$t> for $s {
            type Output = Decimal;

            fn div(self, rhs: $t) -> Self::Output {
                Decimal { repr: self.repr / U512::from(rhs) }
            }
        }

        impl Div<$s> for $t {
            type Output = Decimal;

            fn div(self, rhs: $s) -> Self::Output {
                Decimal::from(self) / rhs
            }
        }

        impl Add<$t> for $s {
            type Output = Decimal;

            fn add(self, rhs: $t) -> Self::Output {
                self + Decimal::from(rhs)
            }
        }

        impl Add<$s> for $t {
            type Output = Decimal;

            fn add(self, rhs: $s) -> Self::Output {
                Decimal::from(self) + rhs
            }
        }

        impl Sub<$t> for $s {
            type Output = Decimal;

            fn sub(self, rhs: $t) -> Self::Output {
                self - Decimal::from(rhs)
            }
        }

        impl Sub<$s> for $t {
            type Output = Decimal;

            fn sub(self, rhs: $s) -> Self::Output {
                Decimal::from(self) - rhs
            }
        }

        impl PartialEq<$t> for $s {
            fn eq(&self, other: &$t) -> bool {
                self.repr == Decimal::from(*other).repr
            }
        }

        impl PartialOrd<$t> for $s {
            fn partial_cmp(&self, other: &$t) -> Option<std::cmp::Ordering> {
                self.repr.partial_cmp(&Decimal::from(*other).repr)
            }
        }
    };
}

impl_int!(u8);
impl_int!(u16);
impl_int!(u32);
impl_int!(u64);
impl_int!(u128);
impl_int!(::primitive_types::U256);
