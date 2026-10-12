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
