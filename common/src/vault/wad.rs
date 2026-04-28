use core::ops::Div;
use std::collections::BTreeMap;
use std::ops::{Add, Sub};

use near_sdk::borsh::schema::{add_definition, Declaration, Definition};
use near_sdk::borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use primitive_types::{U256, U512};
use schemars::JsonSchema;

use crate::schemars::r#gen::SchemaGenerator;
use crate::schemars::schema::Schema;

pub type WIDE = U512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Number(pub U256);

impl Serialize for Number {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        let s = <String as near_sdk::serde::Deserialize>::deserialize(deserializer)?;
        U256::from_dec_str(&s)
            .map(Number)
            .map_err(|_| near_sdk::serde::de::Error::custom("invalid decimal string for U256"))
    }
}

impl Number {
    pub const ZERO: Self = Number(U256([0, 0, 0, 0]));

    #[inline]
    #[must_use]
    pub fn zero() -> Self {
        Self::ZERO
    }

    #[inline]
    #[must_use]
    pub fn one() -> Self {
        Number(U256::one())
    }

    #[inline]
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
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
    pub fn saturating_add(self, other: Number) -> Number {
        Number(self.0.saturating_add(other.0))
    }

    #[inline]
    #[must_use]
    pub fn saturating_sub(self, other: Number) -> Number {
        Number(self.0.saturating_sub(other.0))
    }

    #[inline]
    fn as_u256_trunc(q: U512) -> U256 {
        let mut b64 = [0u8; 64];
        q.write_as_little_endian(&mut b64);
        U256::from_little_endian(&b64[..32])
    }

    #[inline]
    #[must_use]
    pub fn mul_div_floor(multiplicand: Number, multiplier: Number, denominator: Number) -> Number {
        if denominator.is_zero() {
            return Number::zero();
        }
        let product = multiplicand.0.full_mul(multiplier.0);
        let quotient = product / U512::from(denominator.0);
        Number(Self::as_u256_trunc(quotient))
    }

    #[inline]
    #[must_use]
    pub fn mul_div_ceil(multiplicand: Number, multiplier: Number, denominator: Number) -> Number {
        if denominator.is_zero() {
            return Number::zero();
        }
        let product = multiplicand.0.full_mul(multiplier.0);
        let wide_denominator = U512::from(denominator.0);
        let quotient = product / wide_denominator;
        let remainder = product % wide_denominator;
        let base = Number(Self::as_u256_trunc(quotient));
        if remainder.is_zero() {
            base
        } else {
            base.saturating_add(Number::one())
        }
    }
}

impl From<u128> for Number {
    fn from(v: u128) -> Self {
        Number(U256::from(v))
    }
}

impl From<Number> for u128 {
    fn from(n: Number) -> u128 {
        n.as_u128_trunc()
    }
}

impl From<U256> for Number {
    fn from(v: U256) -> Self {
        Number(v)
    }
}

impl From<Number> for U256 {
    fn from(n: Number) -> U256 {
        n.0
    }
}

impl Div<u128> for Number {
    type Output = Number;

    fn div(self, rhs: u128) -> Number {
        Number(self.0 / U256::from(rhs))
    }
}

impl Div<Number> for Number {
    type Output = Number;

    fn div(self, rhs: Number) -> Number {
        Number(self.0 / rhs.0)
    }
}

impl Add<Number> for Number {
    type Output = Number;

    fn add(self, rhs: Number) -> Number {
        Number(self.0 + rhs.0)
    }
}

impl Sub<Number> for Number {
    type Output = Number;

    fn sub(self, rhs: Number) -> Number {
        Number(self.0 - rhs.0)
    }
}

impl BorshSerialize for Number {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut b32 = [0u8; 32];
        self.0.write_as_little_endian(&mut b32);
        writer.write_all(&b32)
    }
}

impl BorshDeserialize for Number {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut b32 = [0u8; 32];
        reader.read_exact(&mut b32)?;
        Ok(Number(U256::from_little_endian(&b32)))
    }
}

impl BorshSchema for Number {
    fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
        add_definition(Self::declaration(), Definition::Primitive(32), definitions);
    }

    fn declaration() -> Declaration {
        "Number".into()
    }
}

impl JsonSchema for Number {
    fn schema_name() -> String {
        "Number".to_string()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut g = generator.subschema_for::<String>().into_object();
        g.metadata().description = Some("256-bit Unsigned Integer".to_string());
        g.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
        g.into()
    }
}

pub const MAX_MANAGEMENT_FEE_WAD: u128 = Wad::SCALE / 100 * 5;
pub const MAX_PERFORMANCE_FEE_WAD: u128 = Wad::SCALE / 100 * 50;
pub const MAX_FEE_WAD: u128 = MAX_PERFORMANCE_FEE_WAD;
pub const YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wad(pub Number);

impl Serialize for Wad {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        Serialize::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Wad {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        <Number as Deserialize>::deserialize(deserializer).map(Wad)
    }
}

impl Wad {
    const SCALE_LOW_LIMB: u64 = 1_000_000_000_000_000_000u64;
    pub const SCALE: u128 = 1_000_000_000_000_000_000u128;
    pub const ZERO: Self = Wad(Number::ZERO);
    pub const ONE: Self = Wad(Number(U256([Self::SCALE_LOW_LIMB, 0, 0, 0])));

    #[inline]
    #[must_use]
    pub fn zero() -> Self {
        Self::ZERO
    }

    #[inline]
    #[must_use]
    pub fn one() -> Self {
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
        self.0 .0 == U256::from(Self::SCALE)
    }

    #[inline]
    #[must_use]
    pub fn as_u128_trunc(self) -> u128 {
        self.0.as_u128_trunc()
    }

    #[inline]
    #[must_use]
    pub fn apply_floored(self, amount: Number) -> Number {
        mul_wad_floor(amount, self)
    }
}

impl From<u128> for Wad {
    fn from(v: u128) -> Self {
        Wad(Number::from(v))
    }
}

impl From<Wad> for u128 {
    fn from(v: Wad) -> Self {
        v.0.as_u128_trunc()
    }
}

impl Div<u128> for Wad {
    type Output = Wad;

    fn div(self, rhs: u128) -> Wad {
        Wad(self.0 / rhs)
    }
}

impl Div<Number> for Wad {
    type Output = Wad;

    fn div(self, rhs: Number) -> Wad {
        Wad(self.0 / rhs)
    }
}

impl BorshSerialize for Wad {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        BorshSerialize::serialize(&self.0, writer)
    }
}

impl BorshDeserialize for Wad {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Number::deserialize_reader(reader).map(Wad)
    }
}

impl BorshSchema for Wad {
    fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
        add_definition(Self::declaration(), Definition::Primitive(32), definitions);
    }

    fn declaration() -> Declaration {
        "Wad".into()
    }
}

impl JsonSchema for Wad {
    fn schema_name() -> String {
        "Wad".to_string()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut g = generator.subschema_for::<String>().into_object();
        g.metadata().description = Some("WAD-scaled U256 (1e18 = 100%)".to_string());
        g.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
        g.into()
    }
}

#[inline]
#[must_use]
pub fn mul_wad_floor(x: Number, y: Wad) -> Number {
    Number::mul_div_floor(x, y.0, Number::from(Wad::SCALE))
}

#[inline]
#[must_use]
pub fn mul_div_floor(x: Number, y: Number, denom: Number) -> Number {
    Number::mul_div_floor(x, y, denom)
}

#[inline]
#[must_use]
pub fn mul_div_ceil(x: Number, y: Number, denom: Number) -> Number {
    Number::mul_div_ceil(x, y, denom)
}

#[must_use]
pub fn compute_fee_shares(
    cur_total_assets: Number,
    last_total_assets: Number,
    fee_wad: Wad,
    total_supply: Number,
) -> Number {
    if cur_total_assets <= last_total_assets || fee_wad.is_zero() || total_supply.is_zero() {
        return Number::zero();
    }

    let profit = cur_total_assets.saturating_sub(last_total_assets);
    let fee_assets = fee_wad.apply_floored(profit);
    compute_fee_shares_from_assets(fee_assets, cur_total_assets, total_supply)
}

#[must_use]
pub fn compute_fee_shares_from_assets(
    fee_assets: Number,
    cur_total_assets: Number,
    total_supply: Number,
) -> Number {
    if fee_assets.is_zero() || cur_total_assets.is_zero() || total_supply.is_zero() {
        return Number::zero();
    }

    let denom = Number(cur_total_assets.0 - fee_assets.0);
    Number::mul_div_floor(fee_assets, total_supply, denom)
}
