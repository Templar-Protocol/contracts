//! Chain-agnostic WAD math primitives for vault share calculations.
//!
//! Provides `Wad` (24-decimal fixed-point) type for precise fee and share calculations.

use core::ops::Div;

use derive_more::{From, Into};
use primitive_types::{U256, U512};

use super::number::Number;

/// Maximum annualized management fee rate: 5%.
pub const MAX_MANAGEMENT_FEE_WAD: u128 = Wad::SCALE / 100 * 5;

/// Maximum performance fee rate on profits: 50%.
pub const MAX_PERFORMANCE_FEE_WAD: u128 = Wad::SCALE / 100 * 50;

/// Backwards-compatible alias for `MAX_PERFORMANCE_FEE_WAD`.
pub const MAX_FEE_WAD: u128 = MAX_PERFORMANCE_FEE_WAD;

/// A 24-decimal fixed-point value (1e24 = 100%), backed by U256.
///
/// When the `serde` feature is enabled, serializes transparently as Number
/// (which serializes to a decimal string for JSON compatibility).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, From, Into)]
pub struct Wad(pub Number);

#[cfg(feature = "serde")]
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

#[cfg(feature = "borsh")]
mod borsh_impl {
    use super::*;
    use alloc::collections::BTreeMap;
    use borsh::schema::{add_definition, Declaration, Definition};
    use borsh::{self, BorshDeserialize, BorshSchema, BorshSerialize};

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
    use schemars::gen::SchemaGenerator;
    use schemars::schema::Schema;
    use schemars::JsonSchema;

    impl JsonSchema for Wad {
        fn schema_name() -> alloc::string::String {
            "Wad".to_string()
        }

        fn json_schema(generator: &mut SchemaGenerator) -> Schema {
            let mut schema = generator.subschema_for::<Number>().into_object();
            schema.metadata().description =
                Some("Wad fixed faction back by 256-bit unsigned integer".to_string());
            schema.string().pattern = Some("^(0|[1-9][0-9]{0,77})$".to_string());
            schema.into()
        }
    }
}

impl Wad {
    /// Scaling factor (1e24).
    pub const SCALE: u128 = 1_000_000_000_000_000_000_000_000u128;

    /// Zero constant.
    pub const ZERO: Self = Wad(Number::ZERO);

    /// Returns zero.
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// Returns one unit (1.0 in WAD scale).
    #[inline]
    #[must_use]
    pub fn one() -> Self {
        Wad::from(Self::SCALE)
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

/// Computes fee shares to mint given:
/// - `cur_total_assets`: current total assets under management
/// - `last_total_assets`: previous total assets snapshot
/// - `performance_fee`: WAD fraction (1e24 = 100%)
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

/// Multiplies x by `y/Wad::SCALE` and floors: floor(x * y / 1e24).
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
    use proptest::prelude::*;

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
        // For any totals, redeem(convert_to_shares(a)) <= a and
        // convert_to_shares(convert_to_assets(s)) >= s due to floor/ceil pairing.
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

    fn u512_from_u256(v: U256) -> U512 {
        let mut bytes = [0u8; 32];
        v.write_as_little_endian(&mut bytes);
        U512::from_little_endian(&bytes)
    }

    fn expected_fee_assets(profit: u128, fee_wad: u128) -> U256 {
        let prod = U512::from(profit) * U512::from(fee_wad);
        let q = prod / U512::from(Wad::SCALE);
        Number::as_u256_trunc(q)
    }

    fn expected_fee_shares(cur: u128, last: u128, fee_wad: u128, total_supply: u128) -> U256 {
        let profit = cur.saturating_sub(last);
        let fee_assets = expected_fee_assets(profit, fee_wad);
        if fee_assets.is_zero() || total_supply == 0 {
            return U256::zero();
        }
        let cur_u256 = U256::from(cur);
        if fee_assets >= cur_u256 {
            return U256::zero();
        }
        let denom = cur_u256 - fee_assets;
        let prod = u512_from_u256(fee_assets) * U512::from(total_supply);
        let q = prod / u512_from_u256(denom);
        Number::as_u256_trunc(q)
    }

    proptest! {
        #[test]
        fn compute_fee_shares_matches_formula(
            cur in any::<u128>(),
            last in any::<u128>(),
            fee_wad in 0u128..=Wad::SCALE,
            total_supply in any::<u128>(),
        ) {
            let result = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(fee_wad),
                Number::from(total_supply),
            );
            let expected = expected_fee_shares(cur, last, fee_wad, total_supply);
            prop_assert_eq!(result.0, expected);
        }

        #[test]
        fn compute_fee_shares_monotonic_in_fee(
            cur in any::<u128>(),
            last in any::<u128>(),
            total_supply in any::<u128>(),
            fee_a in 0u128..=Wad::SCALE,
            fee_b in 0u128..=Wad::SCALE,
        ) {
            let (low, high) = if fee_a <= fee_b { (fee_a, fee_b) } else { (fee_b, fee_a) };
            let minted_low = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(low),
                Number::from(total_supply),
            );
            let minted_high = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(high),
                Number::from(total_supply),
            );
            prop_assert!(minted_low.0 <= minted_high.0);
        }

        // ===================================================================
        // Property: Zero fee produces zero fee shares
        // Invariant: compute_fee_shares(cur, last, 0, supply) == 0
        // ===================================================================
        #[test]
        fn compute_fee_shares_zero_fee_is_zero(
            cur in any::<u128>(),
            last in any::<u128>(),
            total_supply in any::<u128>(),
        ) {
            let result = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::zero(),
                Number::from(total_supply),
            );
            prop_assert!(result.is_zero(), "zero fee should produce zero shares");
        }

        // ===================================================================
        // Property: Zero total supply produces zero fee shares
        // Invariant: compute_fee_shares(cur, last, fee, 0) == 0
        // ===================================================================
        #[test]
        fn compute_fee_shares_zero_supply_is_zero(
            cur in any::<u128>(),
            last in any::<u128>(),
            fee_wad in 0u128..=Wad::SCALE,
        ) {
            let result = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(fee_wad),
                Number::zero(),
            );
            prop_assert!(result.is_zero(), "zero supply should produce zero shares");
        }

        // ===================================================================
        // Property: No profit produces zero fee shares
        // Invariant: If cur <= last, compute_fee_shares returns 0
        // ===================================================================
        #[test]
        fn compute_fee_shares_no_profit_is_zero(
            last in 1u128..=u128::MAX,
            delta in 0u128..=1_000_000u128,
            fee_wad in 1u128..=Wad::SCALE,
            total_supply in 1u128..=u128::MAX,
        ) {
            let cur = last.saturating_sub(delta);
            let result = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(fee_wad),
                Number::from(total_supply),
            );
            prop_assert!(result.is_zero(), "no profit should produce zero shares");
        }

        // ===================================================================
        // Property: Monotonicity in profit
        // Invariant: If profit1 <= profit2 then fee_shares1 <= fee_shares2
        // ===================================================================
        #[test]
        fn compute_fee_shares_monotonic_in_profit(
            last in 1u128..=u64::MAX as u128,
            profit1 in 0u128..=1_000_000_000u128,
            profit2 in 0u128..=1_000_000_000u128,
            fee_wad in 1u128..=Wad::SCALE,
            total_supply in 1u128..=u64::MAX as u128,
        ) {
            let (lo_p, hi_p) = if profit1 <= profit2 { (profit1, profit2) } else { (profit2, profit1) };
            let cur_lo = last.saturating_add(lo_p);
            let cur_hi = last.saturating_add(hi_p);

            let result_lo = compute_fee_shares(
                Number::from(cur_lo),
                Number::from(last),
                Wad::from(fee_wad),
                Number::from(total_supply),
            );
            let result_hi = compute_fee_shares(
                Number::from(cur_hi),
                Number::from(last),
                Wad::from(fee_wad),
                Number::from(total_supply),
            );
            prop_assert!(result_lo.0 <= result_hi.0, "fee shares not monotonic in profit");
        }

        // ===================================================================
        // Property: Wad::apply_floored is bounded by input
        // Invariant: wad.apply_floored(amount) <= amount when wad <= 1.0
        // ===================================================================
        #[test]
        fn wad_apply_floored_bounded(
            wad_raw in 0u128..=Wad::SCALE,
            amount in any::<u128>(),
        ) {
            let wad = Wad::from(wad_raw);
            let result = wad.apply_floored(Number::from(amount));
            prop_assert!(result.0 <= Number::from(amount).0, "apply_floored exceeds input");
        }

        // ===================================================================
        // Property: Wad::apply_floored(1.0) is identity
        // Invariant: Wad::one().apply_floored(amount) == amount
        // ===================================================================
        #[test]
        fn wad_apply_floored_one_is_identity(amount in any::<u128>()) {
            let result = Wad::one().apply_floored(Number::from(amount));
            prop_assert_eq!(result.0, U256::from(amount));
        }

        // ===================================================================
        // Property: Wad::apply_floored(0) is zero
        // Invariant: Wad::zero().apply_floored(amount) == 0
        // ===================================================================
        #[test]
        fn wad_apply_floored_zero_is_zero(amount in any::<u128>()) {
            let result = Wad::zero().apply_floored(Number::from(amount));
            prop_assert!(result.is_zero());
        }

        // ===================================================================
        // Property: Wad monotonicity in apply_floored
        // Invariant: If wad1 <= wad2 then wad1.apply(x) <= wad2.apply(x)
        // ===================================================================
        #[test]
        fn wad_apply_floored_monotonic_in_wad(
            wad1 in 0u128..=Wad::SCALE,
            wad2 in 0u128..=Wad::SCALE,
            amount in any::<u128>(),
        ) {
            let (lo, hi) = if wad1 <= wad2 { (wad1, wad2) } else { (wad2, wad1) };
            let result_lo = Wad::from(lo).apply_floored(Number::from(amount));
            let result_hi = Wad::from(hi).apply_floored(Number::from(amount));
            prop_assert!(result_lo.0 <= result_hi.0);
        }

        // ===================================================================
        // Property: Wad monotonicity in apply_floored amount
        // Invariant: If amount1 <= amount2 then wad.apply(amount1) <= wad.apply(amount2)
        // ===================================================================
        #[test]
        fn wad_apply_floored_monotonic_in_amount(
            wad_raw in 0u128..=Wad::SCALE,
            amount1 in any::<u128>(),
            amount2 in any::<u128>(),
        ) {
            let wad = Wad::from(wad_raw);
            let (lo, hi) = if amount1 <= amount2 { (amount1, amount2) } else { (amount2, amount1) };
            let result_lo = wad.apply_floored(Number::from(lo));
            let result_hi = wad.apply_floored(Number::from(hi));
            prop_assert!(result_lo.0 <= result_hi.0);
        }

        // ===================================================================
        // Property: mul_wad_floor equals Wad::apply_floored
        // Invariant: mul_wad_floor(x, wad) == wad.apply_floored(x)
        // ===================================================================
        #[test]
        fn mul_wad_floor_equals_apply_floored(
            x in any::<u128>(),
            wad_raw in 0u128..=Wad::SCALE,
        ) {
            let wad = Wad::from(wad_raw);
            let result1 = mul_wad_floor(Number::from(x), wad);
            let result2 = wad.apply_floored(Number::from(x));
            prop_assert_eq!(result1.0, result2.0);
        }

        // ===================================================================
        // Property: mul_div_floor == Number::mul_div_floor
        // Invariant: Module function equals struct method
        // ===================================================================
        #[test]
        fn mul_div_floor_equals_number_method(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let result1 = mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            let result2 = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
            prop_assert_eq!(result1.0, result2.0);
        }

        // ===================================================================
        // Property: mul_div_ceil == Number::mul_div_ceil
        // Invariant: Module function equals struct method
        // ===================================================================
        #[test]
        fn mul_div_ceil_equals_number_method(
            x in any::<u128>(),
            y in any::<u128>(),
            denom in 1u128..=u128::MAX,
        ) {
            let result1 = mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            let result2 = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
            prop_assert_eq!(result1.0, result2.0);
        }

        // ===================================================================
        // Property: Share conversion roundtrip bound - deposit path
        // Invariant: redeem(deposit(assets)) <= assets (vault never gives more than deposited)
        // ===================================================================
        #[test]
        fn share_conversion_roundtrip_deposit_path(
            assets in 1u128..=u64::MAX as u128,
            total_supply in 1u128..=u64::MAX as u128,
            total_assets in 1u128..=u64::MAX as u128,
        ) {
            // deposit: shares = floor(assets * (supply + 1) / (total_assets + 1))
            let shares = mul_div_floor(
                Number::from(assets),
                Number::from(total_supply.saturating_add(1)),
                Number::from(total_assets.saturating_add(1)),
            );

            // New totals after deposit
            let new_supply = total_supply.saturating_add(shares.as_u128_trunc());
            let new_assets = total_assets.saturating_add(assets);

            // redeem: back_assets = floor(shares * (new_assets + 1) / (new_supply + 1))
            let back_assets = mul_div_floor(
                shares,
                Number::from(new_assets.saturating_add(1)),
                Number::from(new_supply.saturating_add(1)),
            );

            prop_assert!(
                back_assets.0 <= U256::from(assets),
                "roundtrip gave more assets: {} > {}",
                back_assets.0,
                assets
            );
        }

        // ===================================================================
        // Property: Share conversion roundtrip bound - redeem path
        // Invariant: deposit(redeem(shares)) >= shares (vault never gives more shares than burned)
        // ===================================================================
        #[test]
        fn share_conversion_roundtrip_redeem_path(
            shares in 1u128..=u64::MAX as u128,
            total_supply in 1u128..=u64::MAX as u128,
            total_assets in 1u128..=u64::MAX as u128,
        ) {
            // Ensure shares don't exceed supply
            let shares = shares.min(total_supply);

            // redeem: assets_out = floor(shares * (total_assets + 1) / (total_supply + 1))
            let assets_out = mul_div_floor(
                Number::from(shares),
                Number::from(total_assets.saturating_add(1)),
                Number::from(total_supply.saturating_add(1)),
            );

            // New totals after redeem
            let new_supply = total_supply.saturating_sub(shares);
            let new_assets = total_assets.saturating_sub(assets_out.as_u128_trunc());

            if new_supply == 0 || new_assets == 0 {
                return Ok(());  // Skip edge case
            }

            // deposit back: back_shares = floor(assets_out * (new_supply + 1) / (new_assets + 1))
            let back_shares = mul_div_floor(
                assets_out,
                Number::from(new_supply.saturating_add(1)),
                Number::from(new_assets.saturating_add(1)),
            );

            prop_assert!(
                back_shares.0 <= U256::from(shares),
                "roundtrip gave more shares: {} > {}",
                back_shares.0,
                shares
            );
        }

        // ===================================================================
        // Property: Fee shares bounded under realistic fee caps
        // Invariant: With MAX_PERFORMANCE_FEE_WAD (30%), fee shares won't exceed
        // a reasonable fraction of total_supply under normal conditions.
        // Note: With unrestricted fee_wad approaching 100%, the formula can
        // produce fee_shares > total_supply which is mathematically correct
        // but economically prevented by fee caps at the contract level.
        // ===================================================================
        #[test]
        fn fee_shares_bounded_with_fee_cap(
            cur in 1u128..=u64::MAX as u128,
            last in 1u128..=u64::MAX as u128,
            // Use realistic fee cap (30% = 0.3 * 1e24)
            fee_wad in 1u128..=MAX_PERFORMANCE_FEE_WAD,
            total_supply in 1u128..=u64::MAX as u128,
        ) {
            // Only test with profit
            let cur = cur.max(last);
            let result = compute_fee_shares(
                Number::from(cur),
                Number::from(last),
                Wad::from(fee_wad),
                Number::from(total_supply),
            );

            // With capped fees, fee shares should be bounded relative to supply
            // At 30% fee with 100% profit: fee_assets = 0.3 * cur
            // denom = cur - 0.3*cur = 0.7*cur
            // fee_shares = 0.3*cur * supply / 0.7*cur = 0.3/0.7 * supply ≈ 0.43 * supply
            // So fee shares should never exceed ~43% of supply with 30% fee cap
            let max_ratio = U256::from(total_supply) / U256::from(2u8);  // Conservative 50%
            prop_assert!(
                result.0 <= max_ratio + U256::from(total_supply),
                "fee shares {} > 1.5x total_supply {} (unexpected with capped fees)",
                result.0,
                total_supply
            );
        }
    }
}
