/// Fixed-point helpers and fee-accrual math using 24-decimal WAD precision.
use templar_common::primitive_types::{U256, U512};

pub const WAD: u128 = 1_000_000_000_000_000_000_000_000u128;

// ! FIXME: wrap this in a newtype so we don't mix them around WadFraction(U256)
pub type WADFraction = u128;
pub type WIDE = U512;

/// Multiplies x by y/WAD and floors: floor(x * y / WAD).
/// Typically, y is a WAD-scaled fraction (1e24 = 100%), and x is an unscaled amount.
#[inline]
#[must_use]
pub fn mul_wad_floor(x: u128, y: u128) -> u128 {
    mul_div_floor(x, y, WAD)
}

/// Multiplies and divides with flooring: floor(x * y / denom).
/// Uses 512-bit intermediate (U512) to avoid overflow; returns 0 if denom is 0.
#[inline]
#[must_use]
pub fn mul_div_floor(x: u128, y: u128, denom: u128) -> u128 {
    if denom == 0 {
        return 0;
    }
    let num = WIDE::from(x) * WIDE::from(y);
    let q = num / WIDE::from(denom);
    q.as_u128()
}

/// Multiplies and divides with ceiling: ceil(x * y / denom).
/// Uses 512-bit intermediate (U512) to avoid overflow; returns 0 if denom is 0.
/// Implemented via quotient/remainder to avoid relying on addition overflow behavior.
#[inline]
#[must_use]
pub fn mul_div_ceil(x: u128, y: u128, denom: u128) -> u128 {
    if denom == 0 {
        return 0;
    }
    let num = WIDE::from(x) * WIDE::from(y);
    let d = WIDE::from(denom);
    let q = num / d;
    let r = num % d;
    let base = q.as_u128();
    base.saturating_add((!r.is_zero()) as u128)
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
    cur_total_assets: u128,
    last_total_assets: u128,
    performance_fee: u128,
    total_supply: u128,
) -> u128 {
    if performance_fee == 0 || total_supply == 0 || cur_total_assets <= last_total_assets {
        return 0;
    }
    let profit = cur_total_assets - last_total_assets;
    let fee_assets = mul_wad_floor(profit, performance_fee);
    let denom = cur_total_assets.saturating_sub(fee_assets);

    if denom == 0 {
        return 0;
    }

    if fee_assets == 0 {
        return 0;
    }

    // ERC-4626-like: mint shares so that fee_shares / (total_supply + fee_shares) = fee_assets / cur_total_assets
    // Rearranged and floored:
    mul_div_floor(fee_assets, total_supply, denom)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u128 = WAD;

    #[test]
    fn mul_wad_floor_rounds_down() {
        // 0.3333... * 0.3333... ~= 0.1111...
        let third = W / 3;
        let res = mul_wad_floor(third, third);
        // floor(1/9 * W) = floor(0.111... * 1e24)
        assert!(res <= W / 9);
        assert_eq!(res, (W / 9) - 1); // typical floor loss
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
        let to_sh = mul_div_floor(a, ts + 1, ta + 1);
        let back_a = mul_div_floor(to_sh, ta + 1, ts + 1);
        assert!(back_a <= a);

        let to_a = mul_div_floor(s, ta + 1, ts + 1);
        let back_s = mul_div_ceil(to_a, ts + 1, ta + 1);
        assert!(back_s >= s);
    }

    #[test]
    fn compute_fee_shares_no_profit_or_zero_fee_or_zero_supply() {
        // no profit => 0
        assert_eq!(compute_fee_shares(1_000, 1_000, W / 10, 1_000), 0);
        // zero fee => 0
        assert_eq!(compute_fee_shares(2_000, 1_000, 0, 1_000), 0);
        // zero supply => 0
        assert_eq!(compute_fee_shares(2_000, 1_000, W / 10, 0), 0);
    }

    #[test]
    fn compute_fee_shares_mints_proportionally_on_profit() {
        // cur=1500, last=1000, profit=500, fee=10% => fee_assets=50
        // denom = 1500 - 50 = 1450; total_supply=1000 => fee_shares=floor(50*1000/1450)=34
        let fee = W / 10;
        let minted = compute_fee_shares(1_500, 1_000, fee, 1_000);
        assert_eq!(minted, 34);
    }

    #[test]
    fn compute_fee_shares_handles_extreme_fee() {
        // 100% fee on positive profit: fee_assets=profit; denom=cur_total_assets - fee_assets
        let minted = compute_fee_shares(2_000, 1_000, W, 1_000);
        // fee_assets=1000; denom=1_000 (2_000 - 1_000) => floor(1_000*1_000/1_000)=1_000
        assert_eq!(minted, 1_000);
    }
}
