use core::ops::Div;
use std::collections::BTreeMap;
use std::ops::{Add, Sub};

use near_sdk::borsh::schema::{add_definition, Declaration, Definition, Fields};
use near_sdk::borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use templar_common::primitive_types::{U256, U512};

pub type WIDE = U512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Number(pub U256);

impl Number {
    #[inline]
    pub fn zero() -> Self {
        Number(U256::zero())
    }
    #[inline]
    pub fn one() -> Self {
        Number(U256::one())
    }
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }
    #[inline]
    pub fn is_one(&self) -> bool {
        self.0 == U256::one()
    }
    #[inline]
    pub fn as_u128_trunc(self) -> u128 {
        let mut b32 = [0u8; 32];
        self.0.to_little_endian(&mut b32);
        let mut b16 = [0u8; 16];
        b16.copy_from_slice(&b32[..16]);
        u128::from_le_bytes(b16)
    }
    #[inline]
    fn as_u256_trunc(q: U512) -> U256 {
        let mut b64 = [0u8; 64];
        q.to_little_endian(&mut b64);
        U256::from_little_endian(&b64[..32])
    }
    #[inline]
    pub fn saturating_add(self, other: Number) -> Number {
        Number(self.0.saturating_add(other.0))
    }
    #[inline]
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
        if !r.is_zero() {
            base.saturating_add(Number::one())
        } else {
            base
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
impl From<U256> for Number {
    #[inline]
    fn from(v: U256) -> Self {
        Number(v)
    }
}
impl From<Number> for U256 {
    #[inline]
    fn from(n: Number) -> U256 {
        n.0
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

impl BorshSerialize for Number {
    #[inline]
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut b32 = [0u8; 32];
        self.0.to_little_endian(&mut b32);
        writer.write_all(&b32)
    }
}

impl BorshDeserialize for Number {
    #[inline]
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut b32 = [0u8; 32];
        reader.read_exact(&mut b32)?;
        Ok(Number(U256::from_little_endian(&b32)))
    }
}

/// A 24-decimal fixed-point value (1e24 = 100%), backed by U256.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wad(pub Number);

impl Wad {
    /// Scaling factor (1e24).
    pub const SCALE: u128 = 1_000_000_000_000_000_000_000_000u128;

    /// Returns zero.
    #[inline]
    #[must_use]
    pub fn zero() -> Self {
        Wad(Number::zero())
    }

    /// Returns one unit (1.0 in WAD scale).
    #[inline]
    #[must_use]
    pub fn one() -> Self {
        Wad(Number(U256::from(Self::SCALE)))
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    #[inline]
    pub fn is_one(&self) -> bool {
        self.0 .0 == U256::from(Self::SCALE)
    }

    /// Returns the lower 128 bits (truncation) of this WAD value.
    #[inline]
    #[must_use]
    pub fn as_u128_trunc(self) -> u128 {
        self.0.as_u128_trunc()
    }

    /// Applies this WAD-scaled fraction to an unscaled Number, floored.
    #[inline]
    #[must_use]
    pub fn apply_floored(self, amount: Number) -> Number {
        if amount.is_zero() || self.0.is_zero() {
            return Number::zero();
        }
        let prod = amount.0.full_mul(self.0 .0);
        let q = prod / U512::from(Self::SCALE);
        Number(Number::as_u256_trunc(q))
    }
}

impl From<u128> for Wad {
    #[inline]
    fn from(v: u128) -> Self {
        Wad(Number::from(v))
    }
}

impl From<Wad> for u128 {
    #[inline]
    fn from(w: Wad) -> u128 {
        w.as_u128_trunc()
    }
}

impl Div<u128> for Wad {
    type Output = Wad;
    #[inline]
    fn div(self, rhs: u128) -> Wad {
        Wad(self.0 / rhs)
    }
}
impl Div<Number> for Wad {
    type Output = Wad;
    #[inline]
    fn div(self, rhs: Number) -> Wad {
        Wad(self.0 / rhs)
    }
}

impl BorshSerialize for Wad {
    #[inline]
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        self.0.serialize(writer)
    }
}

impl BorshDeserialize for Wad {
    #[inline]
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let inner = <Number as BorshDeserialize>::deserialize_reader(reader)?;
        Ok(Wad(inner))
    }
}

// FIXME: test these
impl BorshSchema for Number {
    fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
        let definition = Definition::Primitive(32);
        add_definition(Self::declaration(), definition, definitions);
    }

    fn declaration() -> Declaration {
        "Number".into()
    }
}

impl BorshSchema for Wad {
    fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
        let definition = Definition::Primitive(32);
        add_definition(Self::declaration(), definition, definitions);
    }

    fn declaration() -> Declaration {
        "Wad".into()
    }
}

/// Computes fee shares to mint given:
/// - `cur_total_assets`: current total assets under management
/// - `last_total_assets`: previous total assets snapshot
/// - `performance_fee`: WAD fraction (1e24 = 100%)
/// - `total_supply`: current total share supply
///
/// Floors intermediate divisions; returns 0 when no profit, zero fee, zero supply,
/// or when the fee consumes all assets (cur_total_assets == fee_assets).
#[inline]
#[must_use]
pub fn compute_fee_shares(
    cur_total_assets: Number,
    last_total_assets: Number,
    performance_fee: Wad,
    total_supply: Number,
) -> Number {
    if performance_fee.is_zero() || total_supply.is_zero() || cur_total_assets <= last_total_assets
    {
        return Number::zero();
    }
    let profit = cur_total_assets.saturating_sub(last_total_assets);
    if profit.is_zero() {
        return Number::zero();
    }
    let fee_assets = performance_fee.apply_floored(profit);
    if fee_assets.is_zero() {
        return Number::zero();
    }
    if fee_assets.0 >= cur_total_assets.0 {
        return Number::zero();
    }
    let denom = Number(cur_total_assets.0 - fee_assets.0);
    Number::mul_div_floor(fee_assets, total_supply, denom)
}

/// Multiplies x by y/Wad::SCALE and floors: floor(x * y / 1e24).
/// y is a WAD-scaled fraction (1e24 = 100%), and x is an unscaled amount.
#[inline]
#[must_use]
pub fn mul_wad_floor(x: Number, y: Wad) -> Number {
    y.apply_floored(x)
}

/// Multiplies and divides with flooring: floor(x * y / denom).
/// Uses 512-bit intermediate (U512) to avoid overflow; returns 0 if denom is 0.
#[inline]
#[must_use]
pub fn mul_div_floor(x: Number, y: Number, denom: Number) -> Number {
    Number::mul_div_floor(x, y, denom)
}

/// Multiplies and divides with ceiling: ceil(x * y / denom).
/// Uses 512-bit intermediate (U512) to avoid overflow; returns 0 if denom is 0.
/// Implemented via quotient/remainder to avoid relying on addition overflow behavior.
#[inline]
#[must_use]
pub fn mul_div_ceil(x: Number, y: Number, denom: Number) -> Number {
    Number::mul_div_ceil(x, y, denom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_wad_floor_rounds_down() {
        // 0.3333... * 0.3333... ~= 0.1111...
        let third_raw = Number::from(u128::from(Wad::one()) / 3);
        let third = Wad::one() / 3;
        let res = mul_wad_floor(third_raw, third);
        let res_u128: u128 = res.into();
        // floor(1/9 * 1e24)
        assert!(res_u128 <= u128::from(Wad::one()) / 9);
        assert_eq!(res_u128, (u128::from(Wad::one()) / 9) - 1); // typical floor loss
    }

    #[test]
    fn convert_roundtrip_bounds() {
        // For any totals, redeem(convert_to_shares(a)) ≤ a and
        // convert_to_shares(convert_to_assets(s)) ≥ s due to floor/ceil pairing.
        let a = 1_234_567u128;
        let s = 987_654u128;
        // Fake a contract-like environment:
        let ts = 10_000u128;
        let ta = 12_000u128;
        let to_sh: u128 =
            mul_div_floor(Number::from(a), Number::from(ts + 1), Number::from(ta + 1)).into();
        let back_a: u128 = mul_div_floor(
            Number::from(to_sh),
            Number::from(ta + 1),
            Number::from(ts + 1),
        )
        .into();
        assert!(back_a <= a);

        let to_a: u128 =
            mul_div_floor(Number::from(s), Number::from(ta + 1), Number::from(ts + 1)).into();
        let back_s: u128 = mul_div_ceil(
            Number::from(to_a),
            Number::from(ts + 1),
            Number::from(ts + 1),
        )
        .into();
        assert!(back_s >= s);
    }

    #[test]
    fn compute_fee_shares_no_profit_or_zero_fee_or_zero_supply() {
        // no profit => 0
        assert_eq!(
            u128::from(compute_fee_shares(
                Number::from(1_000),
                Number::from(1_000),
                Wad::one() / 10,
                Number::from(1_000)
            )),
            0
        );
        // zero fee => 0
        assert_eq!(
            u128::from(compute_fee_shares(
                Number::from(2_000),
                Number::from(1_000),
                Wad::zero(),
                Number::from(1_000)
            )),
            0
        );
        // zero supply => 0
        assert_eq!(
            u128::from(compute_fee_shares(
                Number::from(2_000),
                Number::from(1_000),
                Wad::one() / 10,
                Number::from(0)
            )),
            0
        );
    }

    #[test]
    fn compute_fee_shares_mints_proportionally_on_profit() {
        // cur=1500, last=1000, profit=500, fee=10% => fee_assets=50
        // denom = 1500 - 50 = 1450; total_supply=1000 => fee_shares=floor(50*1000/1450)=34
        let fee = Wad::one() / 10;
        let minted = compute_fee_shares(
            Number::from(1_500),
            Number::from(1_000),
            fee,
            Number::from(1_000),
        );
        assert_eq!(u128::from(minted), 34);
    }

    #[test]
    fn compute_fee_shares_handles_extreme_fee() {
        // 100% fee on positive profit: fee_assets=profit; denom=cur_total_assets - fee_assets
        let minted = compute_fee_shares(
            Number::from(2_000),
            Number::from(1_000),
            Wad::one(),
            Number::from(1_000),
        );
        // fee_assets=1000; denom=1_000 (2_000 - 1_000) => floor(1_000*1_000/1_000)=1_000
        assert_eq!(u128::from(minted), 1_000);
    }
}
