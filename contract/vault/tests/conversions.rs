use rstest::rstest;
use templar_vault_contract::*;

const W: u128 = WAD;

#[test]
fn no_fee_returns_zero() {
    assert_eq!(compute_fee_shares(1_000, 900, 0, 1_000), 0);
}

#[test]
fn no_profit_returns_zero() {
    assert_eq!(compute_fee_shares(1_000, 1_000, W / 10, 1_000), 0);
    assert_eq!(compute_fee_shares(900, 1_000, W / 10, 1_000), 0);
}

#[test]
fn zero_supply_returns_zero() {
    assert_eq!(compute_fee_shares(1_000, 900, W / 10, 0), 0);
}

#[test]
fn simple_accrual_10_percent_fee() {
    // cur=1200, last=1000, profit=200, fee_assets=20
    // fee_shares = floor(20 * 1000 / (1200-20)) = floor(20000/1180) = 16
    assert_eq!(compute_fee_shares(1200, 1000, W / 10, 1000), 16);
}

#[test]
fn full_fee_100_percent() {
    // cur=1200, last=1000, profit=200, fee_assets=200
    // denom = 1200 - 200 = 1000
    // fee_shares = 200*1000/1000 = 200
    assert_eq!(compute_fee_shares(1200, 1000, W, 1000), 200);
}

// Property: Shares minting never panics, never mints more than `accept` when price ≥ 1
// Model: minted = floor(accept * S / A); price ≥ 1 <=> A >= S => minted ≤ accept
#[rstest(
        accept => [0u128, 1, 2, 10, 1u128<<32, 1u128<<64, u128::MAX/2, u128::MAX-1],
        supply => [0u128, 1, 10, 1u128<<32, 1u128<<64, u128::MAX/2],
        assets_base => [1u128, 2, 10, 1u128<<32, 1u128<<64, u128::MAX/2, u128::MAX-1]
    )]
fn prop_minted_shares_le_accept_when_price_ge_one(accept: u128, supply: u128, assets_base: u128) {
    let assets = assets_base.max(supply); // enforce price ≥ 1
    let minted = mul_div_floor(accept, supply, assets);
    assert!(
        minted <= accept,
        "minted {minted} should be <= accept {accept} when price>=1 (S={supply}, A={assets})"
    );
}

// Property: Fee shares are 0 when not profitable (cur_total_assets <= last_total_assets)
#[rstest(
        perf => [0u128, W/100, W/10],
        last => [0u128, 1u128, 1u128<<32],
        ts => [0u128, 1u128, 1u128<<64]
    )]
fn prop_fee_zero_when_not_profitable(perf: u128, last: u128, ts: u128) {
    let cur_equal = last;
    let cur_lower = last.saturating_sub(1);
    assert_eq!(compute_fee_shares(cur_equal, last, perf, ts), 0);
    assert_eq!(compute_fee_shares(cur_lower, last, perf, ts), 0);
}

#[rstest(
        s =>[0u128, 1, 13, 1<<32, 1<<64],
        a =>[1u128, 7, 1<<32, 1<<64, (1u128<<64) + 123],
        k =>[0u128, 1, 2, 10, 1<<16]
    )]
fn deposit_is_monotone_in_assets(s: u128, a: u128, k: u128) {
    // More assets never produce fewer shares (with fixed totals & offsets).
    let shares1 = mul_div_floor(a, s + 1, a + k + 1);
    let shares2 = mul_div_floor(a + 1, s + 1, a + k + 2);
    assert!(shares2 >= shares1);
}

// Property: Fee shares are monotone =>profit when fee>0 and total_supply>0
#[rstest(
        perf => [W/100, W/10],
        last => [0u128, 1u128<<32],
        ts => [1u128, 1u128<<64],
        p1 => [0u128, 1u128, 1u128<<16],
        p2 => [1u128, 1u128<<16, 1u128<<32]
    )]
fn prop_fee_monotone_in_profit(perf: u128, last: u128, ts: u128, p1: u128, p2: u128) {
    let p_low = p1.min(p2);
    let p_high = p1.max(p2);
    let s1 = compute_fee_shares(last.saturating_add(p_low), last, perf, ts);
    let s2 = compute_fee_shares(last.saturating_add(p_high), last, perf, ts);
    assert!(
        s2 >= s1,
        "fee shares should be monotone =>profit: s2 {s2} >= s1 {s1} (last={last}, perf={perf}, ts={ts})"
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
    fee =>[0u128, W/100, W/10],
    ts =>[0u128, 1, 1<<32, 1<<64],
    last =>[0u128, 1, 1<<32],
    profit =>[0u128, 1, 10, 1<<32]
)]
fn fee_shares_upper_bound_by_total_supply(fee: u128, ts: u128, last: u128, profit: u128) {
    let cur = last.saturating_add(profit);
    let minted = compute_fee_shares(cur, last, fee, ts);
    assert!(minted <= ts || ts == 0);
}
