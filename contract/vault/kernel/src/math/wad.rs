//! Chain-agnostic WAD math primitives for vault share calculations.
//!
//! Provides `Wad` (18-decimal fixed-point) type for precise fee and share calculations.

use core::ops::Div;

use derive_more::{From, Into};
use primitive_types::U256;

use super::number::Number;

/// Maximum annualized management fee rate: 5%.
pub const MAX_MANAGEMENT_FEE_WAD: u128 = Wad::SCALE / 100 * 5;

/// Maximum performance fee rate on profits: 50%.
pub const MAX_PERFORMANCE_FEE_WAD: u128 = Wad::SCALE / 100 * 50;

/// Backwards-compatible alias for `MAX_PERFORMANCE_FEE_WAD`.
pub const MAX_FEE_WAD: u128 = MAX_PERFORMANCE_FEE_WAD;

/// An 18-decimal fixed-point value (1e18 = 100%), backed by U256.
///
/// When the `serde` feature is enabled, serializes transparently as Number
/// (which serializes to a decimal string for JSON compatibility).
#[cfg_attr(not(target_arch = "wasm32"), derive(Debug))]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct Wad(pub Number);

#[cfg(all(feature = "serde", not(feature = "postcard")))]
mod serde_impl {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for Wad {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            // Transparent serialization via Number - use fully qualified syntax
            Serialize::serialize(&self.0, serializer)
        }
    }

    impl<'de> Deserialize<'de> for Wad {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            <Number as Deserialize>::deserialize(deserializer).map(Wad)
        }
    }
}

#[cfg(feature = "postcard")]
mod postcard_serde_impl {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for Wad {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            #[cfg(feature = "soroban")]
            {
                self.0.serialize(serializer)
            }

            #[cfg(not(feature = "soroban"))]
            {
                Serialize::serialize(&self.0, serializer)
            }
        }
    }

    impl<'de> Deserialize<'de> for Wad {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[cfg(feature = "soroban")]
            {
                u128::deserialize(deserializer).map(|value| Wad(Number::from(value)))
            }

            #[cfg(not(feature = "soroban"))]
            {
                <Number as Deserialize>::deserialize(deserializer).map(Wad)
            }
        }
    }
}

#[cfg(feature = "borsh")]
mod borsh_impl {
    use super::*;
    use borsh::{self, BorshDeserialize, BorshSerialize};

    impl BorshSerialize for Wad {
        fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
            BorshSerialize::serialize(&self.0, writer)
        }
    }

    impl BorshDeserialize for Wad {
        fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
            <Number as BorshDeserialize>::deserialize_reader(reader).map(Wad)
        }
    }
}

#[cfg(feature = "borsh-schema")]
mod borsh_schema_impl {
    use super::*;
    use alloc::collections::BTreeMap;
    use borsh::schema::{add_definition, Declaration, Definition};
    use borsh::BorshSchema;

    impl BorshSchema for Wad {
        fn add_definitions_recursively(definitions: &mut BTreeMap<Declaration, Definition>) {
            let definition = Definition::Primitive(32);
            add_definition(Self::declaration(), definition, definitions);
        }

        fn declaration() -> Declaration {
            "Wad".into()
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

    impl JsonSchema for Wad {
        fn schema_name() -> alloc::string::String {
            "Wad".to_string()
        }

        fn json_schema(generator: &mut SchemaGenerator) -> Schema {
            let mut schema = generator.subschema_for::<Number>().into_object();
            schema.metadata().description =
                Some("Wad fixed fraction backed by 256-bit unsigned integer".to_string());
            schema.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
            schema.into()
        }
    }
}

impl Wad {
    /// Scaling factor (1e18).
    pub const SCALE: u128 = 1_000_000_000_000_000_000u128;

    pub const ZERO: Self = Wad(Number::ZERO);
    pub const ONE: Self = Wad(Number(U256([Self::SCALE as u64, 0, 0, 0])));

    /// Returns zero.
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// Returns one unit (1.0 in WAD scale).
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
        Number::mul_div_floor(amount, self.0, Number::from(Self::SCALE))
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

/// Computes fee shares to mint given:
/// - `cur_total_assets`: current total assets under management
/// - `last_total_assets`: previous total assets snapshot
/// - `performance_fee`: WAD fraction (1e18 = 100%)
/// - `total_supply`: current total share supply
///
/// Floors intermediate divisions; returns 0 when no profit, zero fee, zero supply,
/// or when the fee consumes all assets (`cur_total_assets` == `fee_assets`).
#[inline]
#[must_use]
pub fn compute_fee_shares(
    cur_total_assets: Number,
    last_total_assets: Number,
    performance_fee: Wad,
    total_supply: Number,
) -> Number {
    let profit = cur_total_assets.saturating_sub(last_total_assets);
    compute_fee_shares_from_assets(
        performance_fee.apply_floored(profit),
        cur_total_assets,
        total_supply,
    )
}

/// Computes fee shares to mint from a raw `fee_assets` amount, given current total assets and supply.
/// Returns 0 when fee is zero, supply is zero, or fee consumes all assets.
#[inline]
#[must_use]
pub fn compute_fee_shares_from_assets(
    fee_assets: Number,
    cur_total_assets: Number,
    total_supply: Number,
) -> Number {
    if fee_assets.is_zero() || total_supply.is_zero() {
        return Number::zero();
    }
    if fee_assets.0 >= cur_total_assets.0 {
        return Number::zero();
    }
    let denom = Number(cur_total_assets.0 - fee_assets.0);
    Number::mul_div_floor(fee_assets, total_supply, denom)
}

/// Multiplies x by `y/Wad::SCALE` and floors: floor(x * y / 1e18).
/// y is a WAD-scaled fraction (1e18 = 100%), and x is an unscaled amount.
#[inline]
#[must_use]
pub fn mul_wad_floor(x: Number, y: Wad) -> Number {
    Number::mul_div_floor(x, y.0, Number::from(Wad::SCALE))
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

/// Nanoseconds in a standard year (365 days).
pub const YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

/// Compute the effective total_assets for fee accrual, clamping growth
/// to the max rate if configured.
///
/// When `max_rate` is `Some`, limits the effective total_assets to
/// `anchor_total_assets * (1 + max_rate * elapsed / YEAR)`.
#[inline]
#[must_use]
pub fn total_assets_for_fee_accrual(
    cur_total_assets: u128,
    anchor_total_assets: u128,
    anchor_timestamp_ns: u64,
    now_ns: u64,
    max_rate: Option<Wad>,
) -> u128 {
    let Some(max_rate) = max_rate else {
        return cur_total_assets;
    };
    if cur_total_assets <= anchor_total_assets
        || anchor_total_assets == 0
        || now_ns < anchor_timestamp_ns
    {
        return cur_total_assets;
    }
    let elapsed_ns = now_ns - anchor_timestamp_ns;
    if elapsed_ns == 0 {
        return anchor_total_assets;
    }
    let annual_max_increase = max_rate.apply_floored(Number::from(anchor_total_assets));
    let max_increase = mul_div_floor(
        annual_max_increase,
        Number::from(u128::from(elapsed_ns)),
        Number::from(u128::from(YEAR_NS)),
    )
    .as_u128_saturating();
    let max_total_assets = anchor_total_assets.saturating_add(max_increase);
    cur_total_assets.min(max_total_assets)
}

/// Compute management fee shares (time-based fee pro-rated over elapsed time).
///
/// Returns the number of shares to mint for management fees.
#[inline]
#[must_use]
pub fn compute_management_fee_shares(
    fee_assets_base: u128,
    cur_total_assets: u128,
    total_supply: u128,
    management_fee_wad: Wad,
    last_timestamp_ns: u64,
    now_ns: u64,
) -> Number {
    if management_fee_wad.is_zero() || total_supply == 0 || now_ns <= last_timestamp_ns {
        return Number::zero();
    }
    let elapsed_ns = now_ns - last_timestamp_ns;
    let annual_fee_assets = management_fee_wad.apply_floored(Number::from(fee_assets_base));
    let fee_assets = mul_div_floor(
        annual_fee_assets,
        Number::from(u128::from(elapsed_ns)),
        Number::from(u128::from(YEAR_NS)),
    );
    compute_fee_shares_from_assets(
        fee_assets,
        Number::from(cur_total_assets),
        Number::from(total_supply),
    )
}

#[cfg(test)]
mod tests;
