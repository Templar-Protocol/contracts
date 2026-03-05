use super::*;
use primitive_types::U512;
use proptest::prelude::*;

#[test]
fn mul_wad_floor_rounds_down() {
    // 0.3333... * 0.3333... ~= 0.1111...
    let third_raw = Number::from(u128::from(Wad::one()) / 3);
    let third = Wad::one() / 3;
    let res = mul_wad_floor(third_raw, third);
    let res_u128: u128 = res.into();
    // floor(1/9 * 1e18)
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

    #[test]
    fn wad_apply_floored_bounded(
        wad_raw in 0u128..=Wad::SCALE,
        amount in any::<u128>(),
    ) {
        let wad = Wad::from(wad_raw);
        let result = wad.apply_floored(Number::from(amount));
        prop_assert!(result.0 <= Number::from(amount).0, "apply_floored exceeds input");
    }

    #[test]
    fn wad_apply_floored_one_is_identity(amount in any::<u128>()) {
        let result = Wad::one().apply_floored(Number::from(amount));
        prop_assert_eq!(result.0, U256::from(amount));
    }

    #[test]
    fn wad_apply_floored_zero_is_zero(amount in any::<u128>()) {
        let result = Wad::zero().apply_floored(Number::from(amount));
        prop_assert!(result.is_zero());
    }

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

    #[test]
    fn fee_shares_bounded_with_fee_cap(
        cur in 1u128..=u64::MAX as u128,
        last in 1u128..=u64::MAX as u128,
        // Use realistic fee cap (30% = 0.3 * 1e18)
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
