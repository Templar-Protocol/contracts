//! Unit tests for the carry adapter's contract-level guards.
//!
//! Pool-touching behaviour is exercised in `tests/adapter_integration.rs`;
//! this module covers the pure validation gates (constructor, params, oracle
//! freshness/decimals) that hold without a live pool.

use crate::contract::{CarryAdapterContract, CarryAdapterContractClient, CarryError};
use crate::testutils::{CarryFixture, MockOracleContract, MockOracleContractClient};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env, Symbol, Vec};

/// Canonical constructor parameters (valid unless a test overrides one).
type CtorArgs = (
    Address,
    Address,
    Address,
    Address,
    Address,
    Address,
    Address,
    u32,
    u32,
    u32,
    u32,
    u64,
    i128,
);
#[allow(clippy::too_many_arguments)]
fn ctor_args(fx: &CarryFixture, pool: &Address, yield_pool: &Address) -> CtorArgs {
    (
        fx.admin.clone(),
        fx.admin.clone(), // vault: any address for validation tests
        pool.clone(),
        yield_pool.clone(),
        fx.collateral_token.clone(),
        fx.borrow_token.clone(),
        fx.whale.clone(),
        3_000u32,
        50u32,
        0u32,
        50u32,
        86_400u64,
        1_0000000i128,
    )
}

#[test]
fn extend_ttl_is_admin_only() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let adapter_id = env.register(
        CarryAdapterContract,
        ctor_args(&fx, &fx.pool.address, &fx.yield_pool.address),
    );
    let client = CarryAdapterContractClient::new(&env, &adapter_id);
    // Admin call succeeds (host-internal: extends the instance TTL).
    client.extend_ttl(&fx.admin);
    // A non-admin call is rejected.
    let res = client.try_extend_ttl(&Address::generate(&env));
    assert!(res.is_err(), "extend_ttl must be admin-only");
}

#[test]
fn constructor_rejects_same_pool() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.register(
            CarryAdapterContract,
            ctor_args(&fx, &fx.pool.address, &fx.pool.address),
        )
    }));
    // The constructor must reject pool == yield_pool with InvalidInput
    // (surfaced as a host panic); the adapter stays undeployed.
    assert!(outcome.is_err());
}

#[test]
fn set_params_rejects_out_of_bounds() {
    let env = Env::default();
    let fx = CarryFixture::new(&env);
    let adapter_id = env.register(
        CarryAdapterContract,
        ctor_args(&fx, &fx.pool.address, &fx.yield_pool.address),
    );
    let client = CarryAdapterContractClient::new(&env, &adapter_id);
    let bad = crate::contract::Params {
        borrow_utilization_bps: 9_500, // > MAX_BORROW_UTILIZATION_BPS
        surplus_buffer_bps: 50,
        surplus_haircut_bps: 0,
        oracle_free_band_bps: 50,
        oracle_max_age_secs: 86_400,
        min_residual_collateral: 1,
    };
    let res = client.try_set_params(&fx.admin, &bad);
    assert_eq!(res.err(), Some(Ok(CarryError::InvalidParams)));
}

// ---------------------------------------------------------------------------
// Oracle guard (`read_price`'s decimals/freshness/future checks, verified
// against the same SEP-40 mock the adapter reads through).
// ---------------------------------------------------------------------------

use crate::contract::read_price;

#[test]
fn oracle_guard_rejects_wrong_decimals() {
    let env = Env::default();
    let oracle = env.register(MockOracleContract, ());
    let oclient = MockOracleContractClient::new(&env, &oracle);
    oclient.set_data(&Symbol::new(&env, "USD"), &Vec::new(&env), &8, &300);
    let res = read_price(&env, &oracle, &Address::generate(&env), 86_400);
    assert_eq!(res.err(), Some(CarryError::InvalidOracleDecimals));
}

#[test]
fn oracle_guard_rejects_stale_and_future() {
    let env = Env::default();
    env.ledger().set_timestamp(1_800_000_000);
    let oracle = env.register(MockOracleContract, ());
    let asset = Address::generate(&env);
    let oclient = MockOracleContractClient::new(&env, &oracle);
    oclient.set_data(
        &Symbol::new(&env, "USD"),
        &Vec::from_array(&env, [asset.clone()]),
        &7,
        &300,
    );
    // stale: stamped in the past, then the ledger moves beyond the threshold
    oclient.set_price_at(&asset, &1_0000000, &(env.ledger().timestamp() - 1_000));
    env.ledger().set_timestamp(env.ledger().timestamp() + 2_000);
    assert_eq!(
        read_price(&env, &oracle, &asset, 86_400).err(),
        Some(CarryError::OracleStale),
        "stale price must not be served"
    );
    // future: stamped ahead of now, still inside the mock's threshold, so the
    // mock serves it and the contract's own future guard must reject it.
    oclient.set_price_at(&asset, &1_0000000, &(env.ledger().timestamp() + 100));
    assert_eq!(
        read_price(&env, &oracle, &asset, 86_400).err(),
        Some(CarryError::OracleStale),
        "future price must not be served"
    );
    // absent: never priced — lastprice `None` is fail-closed as stale,
    // the same code path as the mock's stale/future rejection.
    assert_eq!(
        read_price(&env, &oracle, &Address::generate(&env), 86_400).err(),
        Some(CarryError::OracleStale)
    );
    // Positive control: fresh price of the right decimals is served.
    oclient.set_price_at(&asset, &1_0000000, &env.ledger().timestamp());
    assert_eq!(read_price(&env, &oracle, &asset, 86_400), Ok(1_0000000));
}
