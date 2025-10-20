use rstest::rstest;
use templar_vault_contract::{wad::compute_fee_shares, *};

#[test]
fn no_fee_returns_zero() {
    assert_eq!(
        compute_fee_shares(1_000.into(), 900.into(), Wad::zero(), 1_000.into()),
        Number::zero()
    );
}

#[test]
fn no_profit_returns_zero() {
    assert_eq!(
        compute_fee_shares(
            1_000.into(),
            1_000.into(),
            Wad::one() / 10u128,
            1_000.into()
        ),
        Number::zero()
    );
    assert_eq!(
        compute_fee_shares(900.into(), 1_000.into(), Wad::one() / 10u128, 1_000.into()),
        Number::zero()
    );
}

#[test]
fn zero_supply_returns_zero() {
    assert_eq!(
        compute_fee_shares(1_000.into(), 900.into(), Wad::one() / 10u128, 0u128.into()),
        Number::zero()
    );
}

#[test]
fn simple_accrual_10_percent_fee() {
    // cur=1200, last=1000, profit=200, fee_assets=20
    // fee_shares = floor(20 * 1000 / (1200-20)) = floor(20000/1180) = 16
    assert_eq!(
        u128::from(compute_fee_shares(
            1200u128.into(),
            1000u128.into(),
            Wad::one() / 10u128,
            1000u128.into()
        )),
        16
    );
}

#[test]
fn full_fee_100_percent() {
    // cur=1200, last=1000, profit=200, fee_assets=200
    // denom = 1200 - 200 = 1000
    // fee_shares = 200*1000/1000 = 200
    assert_eq!(
        u128::from(compute_fee_shares(
            1200u128.into(),
            1000u128.into(),
            Wad::one(),
            1000u128.into()
        )),
        200
    );
}

// Property: Shares minting never panics, never mints more than `accept` when price ≥ 1
// Model: minted = floor(accept * S / A); price ≥ 1 <=> A >= S => minted ≤ accept
#[rstest(
        accept => [0u128.into(), 1u128.into(), 2u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into(), (u128::MAX-1).into()],
        supply => [0u128.into(), 1u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into()],
        assets_base => [1u128.into(), 2u128.into(), 10u128.into(), (1u128<<32).into(), (1u128<<64).into(), (u128::MAX/2).into(), (u128::MAX-1).into()]
    )]
fn prop_minted_shares_le_accept_when_price_ge_one(
    accept: Number,
    supply: Number,
    assets_base: Number,
) {
    let assets = core::cmp::max(assets_base, supply); // enforce price ≥ 1
    let minted = mul_div_floor(accept, supply, assets);
    assert!(
        minted <= accept,
        "minted {minted:?} should be <= accept {accept:?} when price>=1 (S={supply:?}, A={assets:?})"
    );
}

// Property: Fee shares are 0 when not profitable (cur_total_assets <= last_total_assets)
#[rstest(
    perf => [Wad::zero(), Wad::one() / Number::from(100u128), Wad::one() / Number::from(10u128)],
    last => [0u128.into(), 1u128.into(), (1u128<<32).into()],
    ts => [0u128.into(), 1u128.into(), (1u128<<64).into()]
)]
fn prop_fee_zero_when_not_profitable(perf: Wad, last: Number, ts: Number) {
    let cur_equal = last;
    let cur_lower = last.saturating_sub(Number::one());
    assert_eq!(
        compute_fee_shares(cur_equal, last, perf, ts),
        Number::zero()
    );
    assert_eq!(
        compute_fee_shares(cur_lower, last, perf, ts),
        Number::zero()
    );
}

#[rstest(
        s =>[0u128.into(), 1u128.into(), 13u128.into(), (1u128<<32).into(), (1u128<<64).into()],
        a =>[1u128.into(), 7u128.into(), (1u128<<32).into(), (1u128<<64).into(), ((1u128<<64) + 123).into()],
        k =>[0u128.into(), 1u128.into(), 2u128.into(), 10u128.into(), (1u128<<16).into()]
    )]
fn deposit_is_monotone_in_assets(s: Number, a: Number, k: Number) {
    // More assets never produce fewer shares (with fixed totals & offsets).
    let shares1 = mul_div_floor(a, s + Number::one(), a + k + Number::one());
    let shares2 = mul_div_floor(
        a + Number::one(),
        s + Number::one(),
        a + k + Number::from(2u128),
    );
    assert!(shares2 >= shares1);
}

// Property: Fee shares are monotone =>profit when fee>0 and total_supply>0
#[rstest(
        perf => [Wad::one()/100u128, Wad::one()/10u128],
        last => [0u128.into(), (1u128<<32).into()],
        ts => [1u128.into(), (1u128<<64).into()],
        p1 => [0u128.into(), 1u128.into(), (1u128<<16).into()],
        p2 => [1u128.into(), (1u128<<16).into(), (1u128<<32).into()]
    )]
fn prop_fee_monotone_in_profit(perf: Wad, last: Number, ts: Number, p1: Number, p2: Number) {
    let p_low = core::cmp::min(p1, p2);
    let p_high = core::cmp::max(p1, p2);
    let s1 = compute_fee_shares(last.saturating_add(p_low), last, perf, ts);
    let s2 = compute_fee_shares(last.saturating_add(p_high), last, perf, ts);
    assert!(
        s2 >= s1,
        "fee shares should be monotone =>profit: s2 {s2:?} >= s1 {s1:?} (last={last:?}, perf={perf:?}, ts={ts:?})"
    );
}

// Property: Withdrawal math never underflows:
// withdrawn = before - new (saturating)
// credited = min(withdrawn, need)
// remaining = rem - credited (saturating)
#[rstest(
        before => [0u128, 1, 10, 1u128<<64, u128::MAX/2, u128::MAX-1],
        newp => [0u128, 1, 10, 1u128<<64, u128::MAX/2],
        need => [0u128, 1, 10, 1u128<<32, u128::MAX/4],
        rem => [0u128, 1, 10, 1u128<<32, u128::MAX/4]
    )]
fn prop_withdraw_math_never_underflows(before: u128, newp: u128, need: u128, rem: u128) {
    let withdrawn = before.saturating_sub(newp);
    let credited = core::cmp::min(withdrawn, need);
    let remaining = rem.saturating_sub(credited);
    assert!(withdrawn <= before, "withdrawn should not exceed before");
    assert!(credited <= need, "credited should be <= need");
    assert!(remaining <= rem, "remaining should not exceed rem");
}

#[rstest(
    fee =>[Wad::zero(), Wad::one()/100u128, Wad::one()/10u128],
    ts =>[0u128.into(), 1u128.into(), (1u128<<32).into(), (1u128<<64).into()],
    last =>[0u128.into(), 1u128.into(), (1u128<<32).into()],
    profit =>[0u128.into(), 1u128.into(), 10u128.into(), (1u128<<32).into()]
)]
fn fee_shares_upper_bound_by_total_supply(fee: Wad, ts: Number, last: Number, profit: Number) {
    let cur = last.saturating_add(profit);
    let minted = compute_fee_shares(cur, last, fee, ts);
    assert!(minted <= ts || ts.is_zero());
}
