//! NEAR-compatible re-exports of vault math primitives from the kernel.
//!
//! This module provides backward-compatible access to `Wad`, `Number`, and
//! share math functions. All types are re-exported from `templar_vault_kernel`
//! with the `near` feature enabled for NEAR Borsh/Serde compatibility.

// Re-export Number type and WIDE alias
pub use templar_vault_kernel::math::number::{Number, WIDE};

// Re-export Wad type and all math functions
pub use templar_vault_kernel::math::wad::{
    compute_fee_shares, compute_fee_shares_from_assets, mul_div_ceil, mul_div_floor, mul_wad_floor,
    Wad, MAX_FEE_WAD, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wad_json_accepts_decimal_string() {
        let w: Wad = near_sdk::serde_json::from_str("\"50000000000000000000000\"")
            .expect("decimal string should parse");
        assert_eq!(u128::from(w), 50_000_000_000_000_000_000_000u128);
    }

    #[test]
    fn wad_json_rejects_hex_string() {
        let err = near_sdk::serde_json::from_str::<Wad>("\"0xa\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("base-10 digit"), "unexpected error: {msg}");
    }

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
}
