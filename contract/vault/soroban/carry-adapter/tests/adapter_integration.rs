//! Integration tests for `CarryAdapterContract` against the fork-backed
//! `CarryFixture` harness (plan steps 3-5).

use soroban_sdk::testutils::Ledger;
use soroban_sdk::{token::Client as TokenClient, Address, Env};
use templar_soroban_carry_adapter::contract::{
    CarryAdapterContract, CarryAdapterContractClient, CarryError,
};
use templar_soroban_carry_adapter::testutils::CarryFixture;

/// Adapter deployed around a fresh fixture (vault = fixture admin).
struct Adapter<'a> {
    fx: &'a CarryFixture<'a>,
    id: Address,
    client: CarryAdapterContractClient<'a>,
}

impl<'a> Adapter<'a> {
    fn new(fx: &'a CarryFixture<'a>) -> Self {
        let id = fx.env.register(
            CarryAdapterContract,
            (
                fx.admin.clone(),
                fx.admin.clone(), // vault
                fx.pool.address.clone(),
                fx.yield_pool.address.clone(),
                fx.collateral_token.clone(),
                fx.borrow_token.clone(),
                fx.whale.clone(),
                8_000u32, // borrow_utilization_bps
                10u32,    // surplus_buffer_bps
                500u32,   // surplus_haircut_bps
                50u32,    // oracle_free_band_bps
                86_400u64,
                0i128, // min_residual_collateral
            ),
        );
        let client = CarryAdapterContractClient::new(&fx.env, &id);
        Self { fx, id, client }
    }

    /// Vault-style supply of `amount` collateral.
    fn fund(&self, amount: i128) {
        CarryFixture::mint(&self.fx.env, &self.fx.collateral_token, &self.id, amount);
        self.client
            .supply(&self.fx.admin, &self.fx.collateral_token, &amount);
    }

    /// Mint idle borrow asset into the adapter (vault surplus).
    fn mint_idle_b(&self, amount: i128) {
        CarryFixture::mint(&self.fx.env, &self.fx.borrow_token, &self.id, amount);
    }
}

#[test]
fn supply_builds_all_three_legs() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    let pos = a.client.position();
    assert!(pos.c_pool > 0, "collateral supplied");
    assert!(pos.d > 0, "borrow leg opened");
    assert!(pos.y_pool > 0, "yield leg deployed");
    // D <= b_target + 2 units, computed in borrow-asset BASE units:
    // value_b = c_pool * (price_c / price_b) = 100_000_000 * 10_000 = 1e12
    // (10 c @ $10,000 vs $1 b, both 7 decimals), then * c_factor 0.90
    // * l_factor 0.75 * util 80% = 54% -> 540_000000000.
    let value_b = pos.c_pool * 10_000;
    let b_target = value_b * 90 * 75 * 80 / (100 * 100 * 100);
    assert!(pos.d <= b_target + 2, "D {} vs target {}", pos.d, b_target);
}

#[test]
fn ltv_tracks_target_utilization_after_supply() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    let pos = a.client.position();
    assert!(pos.d > 0, "borrow leg opened");
    // The supply leg targets b_target = value_b * c_factor * l_factor *
    // util_bps / 1e4 (floored), so the LTV view —
    // d * 1e4 * 1e14 / (value_b * c_factor * l_factor) — sits at or just
    // under the configured 8_000 bps (rounding: the d-token books debt by
    // ceiling, so the floor-division may show one bps less).
    let value_b = pos.c_pool * 10_000; // 10 c @ $10,000 vs $1 b, both 7 decimals
    let den = value_b * 90 * 75 / (100 * 100);
    let ltv = a.client.ltv();
    assert!(
        (7_999..=8_000).contains(&ltv),
        "ltv {ltv} bps vs util 8000 (d {}, den {})",
        pos.d,
        den
    );
}

#[test]
fn supply_with_stale_oracle_skips_borrow() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    CarryFixture::mint(&env, &fx.collateral_token, &a.id, 10_0000000);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 10_000);
    a.client
        .supply(&fx.admin, &fx.collateral_token, &10_0000000);
    let pos = a.client.position();
    assert!(pos.c_pool > 0);
    assert_eq!(pos.d, 0, "borrow skipped while oracle stale");
    assert_eq!(pos.y_pool, 0);
}

#[test]
fn full_unwind_empties_position() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    let pos = a.client.position();
    let delivered = a
        .client
        .progress_withdrawal(&fx.admin, &fx.collateral_token, &(pos.c + 1));
    assert_eq!(delivered, pos.c);
    let pos2 = a.client.position();
    assert_eq!(pos2.d, 0, "debt cleared");
    assert_eq!(pos2.c_pool, 0, "collateral released");
}

#[test]
fn idle_only_release_delivers_without_pool_submit() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    CarryFixture::mint(&env, &fx.collateral_token, &a.id, 5_0000000);
    let delivered = a
        .client
        .progress_withdrawal(&fx.admin, &fx.collateral_token, &5_0000000);
    assert_eq!(delivered, 5_0000000);
    assert_eq!(a.client.position().c, 0);
}

#[test]
fn sweep_surplus_flags_gates_pays_and_clears() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    // Surplus: 100_0000000 of idle b (buffer keeps 10% of D).
    a.mint_idle_b(100_0000000);
    let before = TokenClient::new(&env, &fx.borrow_token).balance(&fx.whale);
    a.client.sweep_surplus(&fx.admin);
    assert!(a.client.realisation_pending());
    let after = TokenClient::new(&env, &fx.borrow_token).balance(&fx.whale);
    assert!(after > before, "surplus paid to recipient");
    // Fail-closed while pending.
    let res = a
        .client
        .try_supply(&fx.admin, &fx.collateral_token, &1_0000000);
    assert_eq!(res.err(), Some(Ok(CarryError::RealisationPending)));
    let res = a.client.try_total_assets(&fx.collateral_token);
    assert_eq!(res.err(), Some(Ok(CarryError::RealisationPending)));
    // progress_withdrawal still works while pending (idle c only).
    CarryFixture::mint(&env, &fx.collateral_token, &a.id, 1_0000000);
    let delivered = a
        .client
        .progress_withdrawal(&fx.admin, &fx.collateral_token, &1_0000000);
    assert_eq!(delivered, 1_0000000);
    a.client.clear_realisation_pending(&fx.admin);
    assert!(!a.client.realisation_pending());
}

#[test]
fn nav_inside_band_makes_zero_oracle_calls() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    let before = fx.oracle.call_count();
    let _ = a.client.total_assets(&fx.collateral_token);
    assert_eq!(fx.oracle.call_count(), before, "band => zero oracle calls");
}

#[test]
fn nav_outside_band_prices_with_haircut() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    // Surplus must exceed the band threshold (~270_0000000 on this fixture:
    // the borrow leg is sized against the 10_000 USD collateral price).
    a.mint_idle_b(1000_0000000);
    let before = fx.oracle.call_count();
    let nav = a.client.total_assets(&fx.collateral_token);
    assert!(
        fx.oracle.call_count() > before,
        "priced path consulted oracle"
    );
    let pos = a.client.position();
    assert!(nav > pos.c, "surplus added to NAV");
}

#[test]
fn stale_oracle_errors_only_outside_band() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = Adapter::new(&fx);
    a.fund(10_0000000);
    // Inside band: stale oracle changes nothing.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 10_000);
    assert!(a.client.try_total_assets(&fx.collateral_token).is_ok());
    // Outside band (surplus): stale oracle must revert with OracleStale.
    a.mint_idle_b(1000_0000000);
    let res = a.client.try_total_assets(&fx.collateral_token);
    assert_eq!(
        res.err(),
        Some(Ok(CarryError::OracleStale)),
        "stale outside band"
    );
}

// -------------------------------------------------------------------------
// Round-up rounding / zero-mint regression tests (review findings #1207,
// #1219, #1216, constructor same-asset). Scenarios mirror the reviewers'
// harness repros: third-party borrow drives accrual, then the adapter acts.
// -------------------------------------------------------------------------

const E12: i128 = 1_000_000_000_000;
const E7: i128 = 10_000_000;

/// Ceiling division on positive values.
fn ceil_div(a: i128, b: i128) -> i128 {
    (a + b - 1) / b
}

/// Fresh adapter client for `fx` (vault = fixture whale).
fn adapter_client<'a>(fx: &'a CarryFixture<'a>) -> CarryAdapterContractClient<'a> {
    let id = fx.env.register(
        CarryAdapterContract,
        (
            fx.admin.clone(),
            fx.whale.clone(), // vault
            fx.pool.address.clone(),
            fx.yield_pool.address.clone(),
            fx.collateral_token.clone(),
            fx.borrow_token.clone(),
            fx.whale.clone(), // surplus recipient
            8_000u32,
            10u32,
            500u32,
            50u32,
            86_400u64,
            0i128,
        ),
    );
    CarryAdapterContractClient::new(&fx.env, &id)
}

#[test]
fn supply_at_tight_max_util_headroom_after_accrual_succeeds() {
    // #1207: third-party borrow accrues the borrow reserve past 1e12 and the
    // raw max_util headroom (444_080_136_984 for this fixture) books
    // liab' = liab + headroom + 1 > allowed, so the pool reverted the whole
    // supply (InvalidUtilRate). The fixed headroom formula subtracts
    // `min_repay(d_rate) + 1`, so the supply proceeds and the pool's own
    // utilization check holds afterwards.
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = adapter_client(&fx);
    env.mock_all_auths();
    CarryFixture::mint(&env, &fx.collateral_token, &fx.third_party, 100_0000000);
    fx.third_party_supply(
        &fx.pool,
        &fx.third_party,
        &fx.collateral_token,
        100_0000000,
        true,
    );
    fx.third_party_borrow(
        &fx.pool,
        &fx.third_party,
        &fx.third_party,
        &fx.borrow_token,
        4_300_000_000_000,
    );
    fx.advance_time(30 * 24 * 3600);

    let r = fx.reserve(&fx.pool, &fx.borrow_token);
    assert!(r.data.d_rate > E12, "borrow reserve must accrue");

    CarryFixture::mint(&env, &fx.collateral_token, &a.address, 10_0000000);
    let res = a.try_supply(&fx.whale, &fx.collateral_token, &10_0000000);
    assert_eq!(
        res,
        Ok(Ok(())),
        "supply at accrued headroom must succeed: {res:?}"
    );

    // The pool's own utilization check holds after the adapter's borrow.
    let r2 = fx.reserve(&fx.pool, &fx.borrow_token);
    let supply2 = r2.data.b_supply * r2.data.b_rate / E12;
    let liab2 = ceil_div(r2.data.d_supply * r2.data.d_rate, E12);
    let util2 = ceil_div(liab2 * E7, supply2);
    assert!(
        util2 <= i128::from(r2.config.max_util),
        "util2={util2} > max_util"
    );

    // A deleverage of the (now clamped) debt is funded and must not panic.
    assert!(
        a.try_deleverage(&fx.admin).is_ok(),
        "deleverage must not panic"
    );
}

#[test]
fn progress_withdrawal_dust_repay_succeeds_after_accrual() {
    // #1219: FXRP-like cheap collateral (D/C < 1) makes `r = ceil(D*1/C) == 1`
    // a dust repay burning zero d-tokens; pre-fix the pool panicked
    // (InvalidDTokenBurnAmount). Post-fix the repay is raised to the smallest
    // burnable amount and the 1-unit withdrawal proceeds.
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = adapter_client(&fx);
    env.mock_all_auths();
    fx.set_oracle_price(&fx.collateral_token, 5_000_000);
    fx.third_party_supply(
        &fx.pool,
        &fx.third_party,
        &fx.borrow_token,
        50_0000000,
        true,
    );
    fx.third_party_borrow(
        &fx.pool,
        &fx.third_party,
        &fx.third_party,
        &fx.borrow_token,
        10_0000000,
    );
    CarryFixture::mint(&env, &fx.collateral_token, &a.address, 1000_0000000);
    a.supply(&fx.whale, &fx.collateral_token, &1000_0000000);
    fx.advance_time(365 * 24 * 3600);
    fx.refresh_prices();
    fx.set_oracle_price(&fx.collateral_token, 5_000_000);

    let p = a.position();
    let d_rate = fx.reserve(&fx.pool, &fx.borrow_token).data.d_rate;
    let r_units = ceil_div(p.d, p.c);
    let burn = r_units * E12 / d_rate;
    assert!(
        d_rate > E12 && r_units == 1 && burn == 0,
        "dust-repay precondition (r=1, burn=0): d_rate={} C={} D={}",
        d_rate,
        p.c,
        p.d
    );

    let res = a.try_progress_withdrawal(&fx.whale, &fx.collateral_token, &1);
    assert!(res.is_ok(), "dust repay must not panic the pool: {res:?}");
    assert_eq!(res.unwrap().unwrap(), 1, "dust collateral delivered");
    assert!(
        a.try_deleverage(&fx.admin).is_ok(),
        "deleverage after dust withdrawal"
    );
}

#[test]
fn supply_skips_zero_btoken_borrow_leg() {
    // #1216: yield pool accrued (b_rate > 1e12); borrow reserve unaccrued
    // (d_rate == 1e12) with exactly 1 unit of max_util headroom. The raw
    // headroom borrow (b=1) would deploy 1 unit into a yield pool minting 0
    // b-tokens (pool panic). Post-fix the borrow leg is skipped and the
    // collateral leg stands on its own.
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = adapter_client(&fx);
    env.mock_all_auths();
    // Accrue the yield pool: borrow-token collateral + borrow there.
    CarryFixture::mint(&env, &fx.borrow_token, &fx.third_party, 20_000_000_000_000);
    fx.third_party_supply(
        &fx.yield_pool,
        &fx.third_party,
        &fx.borrow_token,
        20_000_000_000_000,
        true,
    );
    fx.third_party_borrow(
        &fx.yield_pool,
        &fx.third_party,
        &fx.third_party,
        &fx.borrow_token,
        10_000_000_000_000,
    );
    fx.advance_time(365 * 24 * 3600);
    let ry = fx.reserve(&fx.yield_pool, &fx.borrow_token);
    assert!(ry.data.b_rate > E12, "yield pool must accrue b_rate > 1e12");

    // Leave exactly 1 unit of max_util headroom on the borrow pool.
    let r = fx.reserve(&fx.pool, &fx.borrow_token);
    assert_eq!(r.data.d_rate, E12, "borrow pool must stay unaccrued");
    let supply = r.data.b_supply * r.data.b_rate / E12;
    let allowed = supply * i128::from(r.config.max_util) / E7;
    CarryFixture::mint(&env, &fx.collateral_token, &fx.third_party, 100_0000000);
    fx.third_party_supply(
        &fx.pool,
        &fx.third_party,
        &fx.collateral_token,
        100_0000000,
        true,
    );
    fx.third_party_borrow(
        &fx.pool,
        &fx.third_party,
        &fx.third_party,
        &fx.borrow_token,
        allowed - 1,
    );

    CarryFixture::mint(&env, &fx.collateral_token, &a.address, 10_0000000);
    let res = a.try_supply(&fx.whale, &fx.collateral_token, &10_0000000);
    assert_eq!(
        res,
        Ok(Ok(())),
        "zero-mint borrow leg must be skipped: {res:?}"
    );
    let pos = a.position();
    assert_eq!(pos.d, 0, "no borrow leg opened");
    assert_eq!(pos.y_pool, 0, "no yield leg deployed");
}

#[test]
fn supply_rejects_dust_collateral_before_submit() {
    // Dust `SupplyCollateral` whose b-token mint rounds to zero is rejected
    // with InvalidInput before any pool submit (pool panic #1216 pre-fix).
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let a = adapter_client(&fx);
    env.mock_all_auths();
    // Accrue the collateral reserve: third-party collateral + collateral
    // borrow, then a year of time.
    CarryFixture::mint(&env, &fx.collateral_token, &fx.third_party, 100_0000000);
    fx.third_party_supply(
        &fx.pool,
        &fx.third_party,
        &fx.collateral_token,
        100_0000000,
        true,
    );
    fx.third_party_borrow(
        &fx.pool,
        &fx.third_party,
        &fx.third_party,
        &fx.collateral_token,
        50_0000000,
    );
    fx.advance_time(365 * 24 * 3600);
    let rc = fx.reserve(&fx.pool, &fx.collateral_token);
    assert!(
        rc.data.b_rate > E12,
        "collateral reserve must accrue b_rate > 1e12"
    );

    CarryFixture::mint(&env, &fx.collateral_token, &a.address, 10_0000000);
    let res = a.try_supply(&fx.whale, &fx.collateral_token, &1);
    // InvalidInput pre-submit, not a pool panic.
    assert!(
        matches!(res, Err(Ok(CarryError::InvalidInput))),
        "dust collateral must be rejected with InvalidInput: {res:?}"
    );
    // Nothing executed: the adapter still holds all collateral, pool empty.
    let pos = a.position();
    assert_eq!(pos.c_pool, 0, "no collateral was supplied");
    assert_eq!(
        TokenClient::new(&env, &fx.collateral_token).balance(&a.address),
        10_0000000,
        "collateral stays with the adapter"
    );
}

#[test]
fn constructor_rejects_collateral_equal_borrow_asset() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.register(
            CarryAdapterContract,
            (
                fx.admin.clone(),
                fx.whale.clone(),
                fx.pool.address.clone(),
                fx.yield_pool.address.clone(),
                fx.borrow_token.clone(),
                fx.borrow_token.clone(),
                fx.whale.clone(),
                8_000u32,
                10u32,
                500u32,
                50u32,
                86_400u64,
                0i128,
            ),
        )
    }));
    assert!(
        outcome.is_err(),
        "collateral_asset == borrow_asset must be rejected"
    );
}
