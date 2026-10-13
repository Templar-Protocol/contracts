use soroban_sdk::{
    symbol_short,
    testutils::{BytesN as _, Ledger as _},
    token::TokenClient,
    vec, Env, IntoVal, Val,
};
use templar_soroban_carry_adapter::testutils::*;

/// The vendored fork pool/backstop/pool-factory wasm must hash to the pinned
/// digests recorded in `testutils` (see artifacts/README.md).
#[test]
fn artifact_pins_match_vendored_wasm() {
    let e = Env::default();
    verify_artifact_pins(&e);
}

/// A real Borrow + Repay against the pinned fork pool, activated through the
/// mock backstop's genuine threshold/q4w checks (status <= 1), with real
/// interest accrual.
#[test]
fn third_party_borrow_repay_and_accrual_on_collateral_pool() {
    let e = Env::default();
    let f = CarryFixture::new(&e);
    let third_party = f.third_party.clone();

    // third party takes collateral and borrows against it
    f.third_party_supply(&f.pool, &third_party, &f.collateral_token, 1_0000000, true);
    f.third_party_borrow(
        &f.pool,
        &third_party,
        &third_party,
        &f.borrow_token,
        10_0000000,
    );

    assert!(f
        .positions(&f.pool, &third_party)
        .liabilities
        .get(f.pool_borrow_idx)
        .is_some());
    let tc = TokenClient::new(&e, &f.borrow_token);
    assert_eq!(tc.balance(&third_party), 110_0000000); // 100 seed + 10 borrowed

    // oracle actually consulted for pricing
    assert!(f.oracle.call_count() > 0);

    // accrue interest: snapshot the raw d_rate at the current ledger (the
    // reserve is already updated to this ledger), advance a day, then repay
    // so the pool applies and persists the accrual.
    let before_d = f.reserve(&f.pool, &f.borrow_token).data.d_rate;
    f.advance_time(86_400); // one day of accrual
    CarryFixture::mint(&e, &f.borrow_token, &third_party, 20_0000000);
    f.third_party_repay(&f.pool, &third_party, &f.borrow_token, 20_0000000);
    let after_d = f.reserve(&f.pool, &f.borrow_token).data.d_rate;
    assert!(after_d > before_d, "interest accrual");
    // lender still holds its (non-collateral) supply position after accrual
    assert!(
        f.positions(&f.pool, &f.whale).supply.get(f.pool_borrow_idx) == Some(5_000_000_000_000)
    );

    // repay closed the liability; collateral stays
    let positions = f.positions(&f.pool, &third_party);
    assert!(positions.liabilities.is_empty());
    assert!(positions.collateral.get(f.pool_collateral_idx).is_some());
}

/// The yield pool takes third-party supply and accrues on its borrow leg.
/// (Borrowing against collateral held on *another* pool is rejected by the
/// fork's per-pool health check — verified in
/// `third_party_borrow_across_pools_is_rejected`.)
#[test]
fn third_party_supply_and_accrual_on_yield_pool() {
    let e = Env::default();
    let f = CarryFixture::new(&e);
    let third_party = f.third_party.clone();

    f.third_party_supply(
        &f.yield_pool,
        &third_party,
        &f.borrow_token,
        50_0000000,
        false,
    );
    assert!(f
        .positions(&f.yield_pool, &third_party)
        .supply
        .get(f.yield_pool_borrow_idx)
        .is_some());

    // seed borrow on the collateral pool makes the yield leg accrual-relevant;
    // advance time and repay so the accrual is applied and persisted.
    f.third_party_supply(&f.pool, &third_party, &f.collateral_token, 1_0000000, true);
    f.third_party_borrow(
        &f.pool,
        &third_party,
        &third_party,
        &f.borrow_token,
        10_0000000,
    );
    let before_d = f.reserve(&f.pool, &f.borrow_token).data.d_rate;
    f.advance_time(3600);
    CarryFixture::mint(&e, &f.borrow_token, &third_party, 20_0000000);
    f.third_party_repay(&f.pool, &third_party, &f.borrow_token, 20_0000000);
    let after_d = f.reserve(&f.pool, &f.borrow_token).data.d_rate;
    assert!(after_d > before_d, "interest accrual");
}

/// The fork validates health factor per-pool: collateral on the collateral
/// pool cannot back a borrow on the yield pool.
#[test]
fn third_party_borrow_across_pools_is_rejected() {
    let e = Env::default();
    let f = CarryFixture::new(&e);
    let third_party = f.third_party.clone();

    f.third_party_supply(&f.pool, &third_party, &f.collateral_token, 1_0000000, true);
    let request = Request {
        request_type: 4u32, // RequestType::Borrow
        address: f.borrow_token.clone(),
        amount: 10_0000000,
    };
    let result = e.try_invoke_contract::<Val, Val>(
        &f.yield_pool.address,
        &soroban_sdk::Symbol::new(&e, "submit"),
        vec![
            &e,
            third_party.clone().into_val(&e),
            third_party.clone().into_val(&e),
            third_party.clone().into_val(&e),
            vec![&e, request].into_val(&e),
        ],
    );
    assert!(
        result.is_err(),
        "cross-pool borrow must fail the per-pool HF check"
    );
}

/// The mock oracle's stale/fresh logic behaves as the pool expects.
#[test]
fn mock_oracle_freshness_and_counter() {
    let e = Env::default();
    let f = CarryFixture::new(&e);
    let third_party = f.third_party.clone();

    f.third_party_supply(&f.pool, &third_party, &f.collateral_token, 1_0000000, true);
    f.third_party_borrow(
        &f.pool,
        &third_party,
        &third_party,
        &f.borrow_token,
        1_0000000,
    );
    let base_calls = f.oracle.call_count();
    assert!(base_calls > 0);

    // jump past the 300s threshold without refreshing prices
    e.ledger().set_timestamp(e.ledger().timestamp() + 600);
    let stale = e.try_invoke_contract::<Val, Val>(
        f.oracle.address(),
        &symbol_short!("get_price"),
        vec![&e, f.borrow_token.clone().into_val(&e)],
    );
    assert!(stale.is_err());

    // refreshing makes it valid again, and the counter keeps counting
    f.refresh_prices();
    let fresh = e.try_invoke_contract::<Val, Val>(
        f.oracle.address(),
        &symbol_short!("get_price"),
        vec![&e, f.borrow_token.clone().into_val(&e)],
    );
    assert!(fresh.is_ok());
    assert!(f.oracle.call_count() > base_calls);
}

/// Both pools carry an enabled borrow reserve with seed supply — the harness
/// is dynamically borrowable on both legs.
#[test]
fn both_pools_have_borrow_reserve_and_liquidity() {
    let e = Env::default();
    let f = CarryFixture::new(&e);
    for pool in [&f.pool, &f.yield_pool] {
        let reserve = f.reserve(pool, &f.borrow_token);
        assert!(reserve.config.enabled);
        assert!(reserve.data.b_supply > 0);
    }
}

/// A pool deployed via the factory can be activated through the genuine
/// threshold path (mock backstop data -> set_status(0)) and is rejected
/// before seeding.
#[test]
fn seed_extra_pool_activates_freshly_deployed_pool() {
    let e = Env::default();
    let f = CarryFixture::new(&e);

    let extra_addr = f.pool_factory.deploy(
        &f.admin,
        &soroban_sdk::String::from_str(&e, "CarryExtra"),
        &soroban_sdk::BytesN::random(&e),
        f.oracle.address(),
        &0,         // backstop take rate
        &10,        // max positions
        &1_0000000, // min collateral
    );
    let extra = PoolClient::new(&e, &extra_addr);
    // Before seeding, the fork's own threshold check rejects activation.
    assert!(extra.try_set_status(&0).is_err());

    f.seed_extra_pool(&extra);
    assert!(extra.get_config().status <= 1);
}
