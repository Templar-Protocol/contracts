//! Vault ↔ carry-adapter end-to-end (plan step 7, cases (a)-(f)).
//!
//! The vault and the carry adapter are registered natively in the same host
//! process (same pattern as `blend_e2e.rs`). Failing cases MUST go through a
//! host frame (`env.try_invoke_contract`) and assert that the vault asset
//! balance, `pool.get_positions(adapter)` and the yield-pool positions are
//! unchanged afterwards: the in-process frame commits partial effects on
//! failure, so a panicking (non-`try_`) call would leave the pool in a
//! partially-applied state that a unit-test-style assertion could not see.
//!
//! Kernel error mapping: the vault kernel collapses every
//! `invalid_state_error(...)` — including `supply observation out of bounds`,
//! `refresh observation exceeds market cap` and `principal underflow on
//! withdraw` — into `ContractError::InvalidState`, so tests assert the
//! `InvalidState` result plus the unchanged state; the *cause* is discriminated
//! by the complementary positive case inside each test (larger supply passes,
//! raised cap refreshes, refreshed principal withdraws).

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::Client as TokenClient,
    Address, Bytes, Env, IntoVal, Symbol,
};
use std::string::String as AllocString;
use std::vec::Vec;
use templar_curator_primitives::MarketConfig;
use templar_soroban_carry_adapter::contract::{CarryAdapterContract, CarryAdapterContractClient};
use templar_soroban_carry_adapter::testutils::{CarryFixture, Positions};
use templar_soroban_governance::SorobanVaultGovernanceContract;
use templar_soroban_runtime::{
    contract::SorobanVaultContract,
    storage::{SorobanStorage, Storage},
    ContractError,
};
use templar_soroban_shared_types::{
    GovernanceCommand, I128Receipt, VaultCommand, GOVERNANCE_CONFIG_KIND_ALLOCATORS,
    GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS, GOVERNANCE_POLICY_KIND_CAP,
    GOVERNANCE_POLICY_KIND_REMOVE_MARKET, GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
};

/// One c unit (collateral has 7 decimals); also the borrow-asset unit.
const UNIT: i128 = 1_0000000;

/// Adapter parameters: (util_bps, buffer_bps, haircut_bps, band_bps).
type Params = (u32, u32, u32, u32);
const DEFAULTS: Params = (8_000, 10, 500, 50);

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

/// Addresses only — the `Env` itself is passed to each helper explicitly.
struct Ctx {
    fixture_admin: Address,
    vault: Address,
    governance: Address,
    allocator: Address,
    user: Address,
    adapter: Address,
}

fn wire(address: &Address) -> AllocString {
    AllocString::from_utf8(address.to_string().to_bytes().to_alloc_vec()).expect("valid address")
}

fn setup(env: &Env, fixture: &CarryFixture, params: Params) -> Ctx {
    // The fork-pool interactions (health checks, submit batches) exceed the
    // default test budget; artifact fixtures use the same unlimited budget.
    env.cost_estimate().budget().reset_unlimited();
    let vault = env.register(SorobanVaultContract, ());
    let governance = env.register(
        SorobanVaultGovernanceContract,
        (&fixture.admin, &vault, &(0u64)),
    );
    let share = env
        .register_stellar_asset_contract_v2(vault.clone())
        .address();
    env.as_contract(&vault, || {
        SorobanVaultContract::initialize(
            env.clone(),
            fixture.admin.clone(),
            governance.clone(),
            fixture.collateral_token.clone(),
            share,
            0,
            0,
        )
        .unwrap();
    });

    let allocator = Address::generate(env);
    governance_config(
        env,
        &vault,
        &governance,
        GOVERNANCE_CONFIG_KIND_ALLOCATORS,
        &[allocator.clone()],
    );

    let (util, buffer, haircut, band) = params;
    let adapter = env.register(
        CarryAdapterContract,
        (
            fixture.admin.clone(), // adapter admin (converter/curator domain)
            vault.clone(),
            fixture.pool.address.clone(),
            fixture.yield_pool.address.clone(),
            fixture.collateral_token.clone(),
            fixture.borrow_token.clone(),
            fixture.admin.clone(), // surplus recipient / converter
            util,
            buffer,
            haircut,
            band,
            86_400u64,
            0i128, // min_residual_collateral
        ),
    );

    governance_config(
        env,
        &vault,
        &governance,
        GOVERNANCE_CONFIG_KIND_ALLOWED_ADAPTERS,
        &[adapter.clone()],
    );
    env.as_contract(&vault, || {
        let mut storage = SorobanStorage::new(env);
        let mut policy_state = storage.load_policy_state().unwrap().unwrap_or_default();
        policy_state
            .set_market_config(0, MarketConfig::new(true, i128::MAX as u128, None))
            .unwrap();
        storage.save_policy_state(&policy_state).unwrap();
    });
    env.as_contract(&vault, || {
        run_governance_command(
            env,
            &governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_SUPPLY_QUEUE,
                target_ids: Some(vec![0u32]),
                mode: None,
                accounts: Some(vec![wire(&adapter)]),
                market_id: None,
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        )
        .unwrap();
    });

    let user = Address::generate(env);
    CarryFixture::mint(env, &fixture.collateral_token, &user, 200_0000000);

    Ctx {
        fixture_admin: fixture.admin.clone(),
        vault,
        governance,
        allocator,
        user,
        adapter,
    }
}

/// Runs a governance command from inside the vault contract (governance
/// preauthorized path — the same pattern `blend_e2e.rs` uses).
fn run_governance_command(
    env: &Env,
    governance_addr: &Address,
    command: &GovernanceCommand,
) -> Result<(), ContractError> {
    let payload = Bytes::from_slice(env, &command.encode());
    SorobanVaultContract::execute_governance(env.clone(), governance_addr.clone(), payload)
}

fn governance_config(
    env: &Env,
    vault: &Address,
    governance_addr: &Address,
    kind: u32,
    many: &[Address],
) {
    env.as_contract(vault, || {
        run_governance_command(
            env,
            governance_addr,
            &GovernanceCommand::SetGovernanceConfig {
                kind,
                primary: None,
                many: Some(many.iter().map(wire).collect::<Vec<AllocString>>()),
                value_a: None,
                value_b: None,
            },
        )
        .unwrap();
    });
}

fn governance_cap(env: &Env, ctx: &Ctx, cap: i128) {
    env.as_contract(&ctx.vault, || {
        run_governance_command(
            env,
            &ctx.governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_CAP,
                target_ids: None,
                mode: None,
                accounts: None,
                market_id: Some(0),
                cap_group_id: None,
                value: Some(cap),
                value_b: None,
                value_c: None,
            },
        )
        .unwrap();
    });
}

fn governance_remove_market(env: &Env, ctx: &Ctx) {
    env.as_contract(&ctx.vault, || {
        run_governance_command(
            env,
            &ctx.governance,
            &GovernanceCommand::SetGovernancePolicy {
                kind: GOVERNANCE_POLICY_KIND_REMOVE_MARKET,
                target_ids: None,
                mode: None,
                accounts: None,
                market_id: Some(0),
                cap_group_id: None,
                value: None,
                value_b: None,
                value_c: None,
            },
        )
        .unwrap();
    });
}

// ---------------------------------------------------------------------------
// Vault host frames
// ---------------------------------------------------------------------------

fn try_execute(env: &Env, vault: &Address, command: &VaultCommand) -> Result<Bytes, ContractError> {
    match env.try_invoke_contract::<Bytes, ContractError>(
        vault,
        &Symbol::new(env, "execute"),
        (&Bytes::from_slice(env, &command.encode()),).into_val(env),
    ) {
        Ok(Ok(bytes)) => Ok(bytes),
        Err(Ok(error)) => Err(error),
        Err(Err(host)) => panic!("unexpected host error: {host:?}"),
        Ok(Err(val)) => panic!("unexpected non-error failure: {val:?}"),
    }
}

fn execute(env: &Env, vault: &Address, command: &VaultCommand) -> Bytes {
    try_execute(env, vault, command)
        .unwrap_or_else(|e| panic!("vault execute failed: {e:?} cmd={}", command_name(command)))
}

/// Diagnostic name for panics (test-only).
fn command_name(command: &VaultCommand) -> &'static str {
    match command {
        VaultCommand::DepositWithMin { .. } => "DepositWithMin",
        VaultCommand::Allocate { .. } => "Allocate",
        VaultCommand::RefreshMarkets { .. } => "RefreshMarkets",
        VaultCommand::AtomicRedeem { .. } => "AtomicRedeem",
        _ => "other",
    }
}

fn decode_i128(bytes: &Bytes) -> i128 {
    I128Receipt::decode(&bytes.to_alloc_vec())
        .expect("I128 receipt")
        .value
}

fn deposit(env: &Env, ctx: &Ctx, amount: i128) {
    execute(
        env,
        &ctx.vault,
        &VaultCommand::DepositWithMin {
            owner: wire(&ctx.user),
            receiver: wire(&ctx.user),
            assets: amount,
            min_shares_out: 0,
        },
    );
}

fn try_allocate(env: &Env, ctx: &Ctx, amount: i128, supply: bool) -> Result<Bytes, ContractError> {
    try_execute(
        env,
        &ctx.vault,
        &VaultCommand::Allocate {
            caller: wire(&ctx.allocator),
            market: 0,
            amount,
            supply,
        },
    )
}

fn allocate(env: &Env, ctx: &Ctx, amount: i128, supply: bool) -> i128 {
    decode_i128(&try_allocate(env, ctx, amount, supply).expect("allocate succeeds"))
}

fn try_refresh(env: &Env, ctx: &Ctx) -> Result<Bytes, ContractError> {
    try_execute(
        env,
        &ctx.vault,
        &VaultCommand::RefreshMarkets {
            caller: wire(&ctx.allocator),
            markets: vec![0u32],
        },
    )
}

fn refresh(env: &Env, ctx: &Ctx) -> i128 {
    decode_i128(&try_refresh(env, ctx).expect("refresh succeeds"))
}

// ---------------------------------------------------------------------------
// State snapshots
// ---------------------------------------------------------------------------

fn vault_balance(env: &Env, fixture: &CarryFixture, who: &Address) -> i128 {
    TokenClient::new(env, &fixture.collateral_token).balance(who)
}

/// `(total_shares, idle_assets, external_assets, total_assets)`.
fn vault_kernel_state(env: &Env, vault: &Address) -> (i128, i128, i128, i128) {
    env.as_contract(vault, || {
        SorobanVaultContract::proxy_view(env.clone(), Address::generate(env), 0, 0)
            .unwrap()
            .0
             .2
    })
}

/// `(total_shares, convert_to_assets(total_shares))` — NAV per full share.
fn vault_price(env: &Env, vault: &Address) -> (i128, i128) {
    env.as_contract(vault, || {
        let (total_shares, _, _, _) = vault_kernel_state(env, vault);
        let probe = if total_shares > 0 { total_shares } else { 0 };
        let core = SorobanVaultContract::proxy_view(env.clone(), Address::generate(env), 0, probe)
            .unwrap();
        (total_shares, core.2 .1)
    })
}

/// Vault asset balance + both-pool positions + kernel state.
#[derive(Debug, PartialEq)]
struct Snapshot {
    vault_balance: i128,
    pool_supply: Vec<(u32, i128)>,
    pool_liabilities: Vec<(u32, i128)>,
    yield_supply: Vec<(u32, i128)>,
    kernel: (i128, i128, i128, i128),
}

fn positions_map(map: &soroban_sdk::Map<u32, i128>) -> Vec<(u32, i128)> {
    let mut out: Vec<(u32, i128)> = Vec::new();
    for key in map.keys() {
        if let Some(value) = map.get(key) {
            out.push((key, value));
        }
    }
    out.sort();
    out
}

fn snapshot(env: &Env, fixture: &CarryFixture, ctx: &Ctx) -> Snapshot {
    let pool: Positions = fixture.pool.get_positions(&ctx.adapter);
    let ypool: Positions = fixture.yield_pool.get_positions(&ctx.adapter);
    Snapshot {
        vault_balance: vault_balance(env, fixture, &ctx.vault),
        pool_supply: positions_map(&pool.supply),
        pool_liabilities: positions_map(&pool.liabilities),
        yield_supply: positions_map(&ypool.supply),
        kernel: vault_kernel_state(env, &ctx.vault),
    }
}

// ---------------------------------------------------------------------------
// Test-level helpers
// ---------------------------------------------------------------------------

/// Standard fund: deposit then the canonical refresh/allocate pair.
fn fund(env: &Env, _fixture: &CarryFixture, ctx: &Ctx, amount: i128) {
    deposit(env, ctx, amount);
    allocate(env, ctx, amount, true);
}

/// Drives negative drift: the third party collateralizes with the COLLATERAL
/// token (same-token supply is not accepted as collateral — Blend #1205) and
/// borrows borrow-token on the MAIN pool, pushing utilization to ~90% so the
/// borrow APR surges and the adapter's debt `D` accrues far past the band
/// while the yield leg `Y` sits near-idle on the yield pool.
fn third_party_borrow_on_main_pool(env: &Env, fixture: &CarryFixture, supply: i128, borrow: i128) {
    CarryFixture::mint(env, &fixture.collateral_token, &fixture.third_party, supply);
    fixture.third_party_supply(
        &fixture.pool,
        &fixture.third_party,
        &fixture.collateral_token,
        supply,
        true,
    );
    fixture.third_party_borrow(
        &fixture.pool,
        &fixture.third_party,
        &fixture.third_party,
        &fixture.borrow_token,
        borrow,
    );
}

/// Converter-side donation: b surplus converted to c, transferred to the
/// adapter (the vault sees it as idle collateral on the next NAV read).
fn donate_converted_c(env: &Env, fixture: &CarryFixture, ctx: &Ctx, b_surplus: i128, c_usd: i128) {
    // Oracle: borrow (b) == 1 USD, collateral (c) == `c_usd` USD, so
    // `c_usd` b units convert to 1 c unit; donate 95% of that (the
    // surplus haircut side) so the vault books value, not more than swept.
    let c_amount = b_surplus * 95 / (100 * c_usd);
    CarryFixture::mint(env, &fixture.collateral_token, &ctx.adapter, c_amount);
}

// ---------------------------------------------------------------------------
// (a) refresh then supply in the same ledger succeeds
// ---------------------------------------------------------------------------

#[test]
fn refresh_then_supply_in_same_ledger_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);

    deposit(&env, &ctx, 10_0000000);

    // Canonical curator protocol: refresh, then allocate — adjacent, same
    // ledger. The supply leg (collateral + borrow + yield) executes.
    refresh(&env, &ctx);
    let external = allocate(&env, &ctx, 10_0000000, true);
    assert_eq!(external, 10_0000000);

    // The market is live on the pool.
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    let position = adapter.position();
    assert!(position.c_pool > 0, "collateral supplied to pool");
    assert!(position.d > 0, "carry position carries debt");
    assert!(position.y > 0, "yield leg deployed");

    // NAV confirms principal.
    assert_eq!(refresh(&env, &ctx), 10_0000000);
    let (_, _, external_assets, total_assets) = vault_kernel_state(&env, &ctx.vault);
    assert_eq!(total_assets, 10_0000000);
    assert_eq!(external_assets, 10_0000000);
}

// ---------------------------------------------------------------------------
// (b) negative drift: supply smaller than the shortfall fails; larger passes
// ---------------------------------------------------------------------------

#[test]
fn negative_drift_supply_must_cover_shortfall() {
    let env = Env::default();
    env.mock_all_auths();
    // oracle_free_band_bps = 50 bps of D; the borrow-APR surge dwarfs the
    // band, so the drift is priced into NAV.
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    fund(&env, &fixture, &ctx, 10_0000000);

    // Make D accrue past Y: the third party collateralizes with the
    // collateral token (same-token supply is not accepted as collateral —
    // Blend #1205), borrows on the MAIN pool, and a year of accrual runs
    // (the yield leg sits near-idle on the yield pool; advance_time
    // refreshes the mock-oracle freshness stamps).
    third_party_borrow_on_main_pool(&env, &fixture, 1_0000000, 10_0000000);
    fixture.advance_time(365 * 86_400);

    // `total_assets` reflects the accrued shortfall WITHOUT the kernel
    // re-anchoring its principal — `RefreshMarkets` would book the loss
    // into principal immediately (plan: principal re-anchoring is a
    // refresh/supply observation effect). Measure the shortfall from the
    // adapter view only.
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    let nav = adapter.total_assets(&fixture.collateral_token);
    let shortfall = 10_0000000 - nav;
    assert!(shortfall >= 2, "expected negative drift, nav {nav}");

    // A supply SMALLER than the shortfall: observed principal drops below
    // the stored principal -> `supply observation out of bounds`, host
    // frame reverts with balances/positions unchanged. (Deposit first:
    // the kernel transfers from the vault's balance.)
    deposit(&env, &ctx, shortfall - 1);
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_allocate(&env, &ctx, shortfall - 1, true)
        .expect_err("supply under an uncovered shortfall must fail");
    assert_eq!(err, ContractError::InvalidState);
    // The host frame rolled back: pool/yield-pool positions and the
    // kernel's stored principal are untouched (the deposit ran earlier
    // and stayed idle in the vault).
    assert_eq!(
        snapshot(&env, &fixture, &ctx),
        before,
        "host frame rolled back"
    );
    // The failed frame rolled back: principal stayed at 10_0000000 and the
    // deposit's collateral sat idle in the vault (total_assets = idle +
    // principal; the kernel's `total_assets` field is idle + external, and
    // the external leg still carries the unbooked shortfall).
    let (_, kernel_idle, _, kernel_total) = vault_kernel_state(&env, &ctx.vault);
    assert_eq!(
        kernel_idle,
        shortfall - 1,
        "the failed frame rolled back; principal untouched"
    );
    let _ = kernel_total;

    // A LARGER supply passes and books the loss into principal. The failed
    // host frame left the first deposit idle in the vault; top up to exactly
    // the new supply so idle stays zero afterwards.
    let supply = shortfall + 5_0000000;
    deposit(&env, &ctx, supply - (shortfall - 1));
    let external = allocate(&env, &ctx, supply, true);
    assert_eq!(external, 10_0000000 - shortfall + supply);
    let (_, _, _, total_assets) = vault_kernel_state(&env, &ctx.vault);
    assert_eq!(total_assets, external, "loss booked into principal");
    assert_eq!(refresh(&env, &ctx), external);
}

// ---------------------------------------------------------------------------
// (b') positive drift: surplus-side |Y-D| outside the band => supply fails
// ---------------------------------------------------------------------------

#[test]
fn positive_drift_supply_out_of_band_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    fund(&env, &fixture, &ctx, 10_0000000);
    let nav_before = refresh(&env, &ctx);

    // Surplus side drift: raw b donated to the adapter (the converter
    // returning change). The donation must exceed the oracle-free band
    // (`oracle_free_band_bps * D`) so the surplus is actually priced into
    // NAV; NAV then grows outside the kernel's supply window.
    CarryFixture::mint(&env, &fixture.borrow_token, &ctx.adapter, 1_000 * UNIT);
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    assert!(
        adapter.total_assets(&fixture.collateral_token) > nav_before,
        "donation grows NAV"
    );

    // Supply between the NAV jump and the refresh fails out-of-band.
    // (Deposit first: the kernel transfers from the vault's balance.)
    deposit(&env, &ctx, 2);
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_allocate(&env, &ctx, 2, true).expect_err("supply into positive drift must fail");
    assert_eq!(err, ContractError::InvalidState);
    assert_eq!(
        snapshot(&env, &fixture, &ctx),
        before,
        "host frame rolled back"
    );

    // Curator protocol recovers: refresh first, then supply (the deposit was
    // restored by the host-frame rollback).
    refresh(&env, &ctx);
    allocate(&env, &ctx, 2, true);
}

// ---------------------------------------------------------------------------
// (c) tight cap: NAV > cap fails refresh; wind-down clears the market
// ---------------------------------------------------------------------------

#[test]
fn tight_cap_blocks_refresh_and_wind_down_clears_market() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    fund(&env, &fixture, &ctx, 10_0000000);

    // NAV grows above a tight cap via surplus-side drift (converter top-up).
    CarryFixture::mint(&env, &fixture.borrow_token, &ctx.adapter, 100 * UNIT);
    let nav = refresh(&env, &ctx);
    let cap = nav / 2;
    assert!(cap > 0, "cap must be positive for the negative test");
    governance_cap(&env, &ctx, cap);

    // refresh: NAV > cap -> `refresh observation exceeds market cap`.
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_refresh(&env, &ctx).expect_err("refresh above cap must fail");
    assert_eq!(err, ContractError::InvalidState);
    assert_eq!(
        snapshot(&env, &fixture, &ctx),
        before,
        "host frame rolled back"
    );

    // Wind-down order: deleverage (price-free repay), sweep surplus,
    // converter donates c to the adapter, admin clears pending, refresh,
    // withdraw full principal, (second sweep if Y > 0), refresh to 0,
    // cap 0, remove market.
    if adapter.position().d > 0 {
        adapter.deleverage(&ctx.fixture_admin);
    }
    assert_eq!(adapter.position().d, 0, "debt repaid price-free");
    adapter.sweep_surplus(&ctx.fixture_admin);
    assert!(adapter.realisation_pending());

    // Inside the window the vault cannot price; deposit runs at the stored
    // principal (see (f)); here the wind-down just waits for the donation.
    // The kernel surfaces the adapter's RealisationPending host code (#11)
    // verbatim — the vault's own ContractError::VaultNotIdle carries the
    // same code 11 — and plan A6(f)'s observable is the failure plus the
    // unchanged snapshot asserted here.
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_refresh(&env, &ctx).expect_err("refresh while pending must fail");
    assert_eq!(
        err,
        ContractError::VaultNotIdle,
        "adapter #11 surfaces unchanged"
    );
    assert_eq!(snapshot(&env, &fixture, &ctx), before);

    donate_converted_c(&env, &fixture, &ctx, 100 * UNIT, 10_000);
    adapter.clear_realisation_pending(&ctx.fixture_admin);

    // The tight cap still blocks pricing above it, so the wind-down
    // continues at the STORED principal: withdraw the full principal
    // (idle-first delivery consumes the donated c first; D == 0 so no
    // repayment rides along).
    let external = allocate(&env, &ctx, 10_0000000, false);
    assert_eq!(external, 0, "external drains to 0 at the stored principal");

    // Refresh books the residual converted surplus (below the cap) and the
    // wind-down withdraws it too.
    let leftover = refresh(&env, &ctx);
    if leftover > 0 {
        allocate(&env, &ctx, leftover, false);
    }

    // A second sweep is possible while Y (buffered leftovers) > 0.
    let pos = adapter.position();
    if pos.y > 0 {
        adapter.sweep_surplus(&ctx.fixture_admin);
        donate_converted_c(&env, &fixture, &ctx, pos.y, 10_000);
        adapter.clear_realisation_pending(&ctx.fixture_admin);
        let rest = refresh(&env, &ctx);
        if rest > 0 {
            allocate(&env, &ctx, rest, false);
        }
    }

    assert_eq!(adapter.position().c, 0, "no collateral left");
    assert_eq!(adapter.position().d, 0);

    assert_eq!(refresh(&env, &ctx), 0, "wind-down ends at NAV 0");
    governance_cap(&env, &ctx, 0);
    governance_remove_market(&env, &ctx);
    let policy = env.as_contract(&ctx.vault, || {
        SorobanStorage::new(&env)
            .load_policy_state()
            .unwrap()
            .unwrap()
    });
    assert_eq!(policy.principal_for(0).unwrap_or(0), 0, "principal cleared");
    assert!(policy.market_config(0).is_none(), "market removed");
}

// ---------------------------------------------------------------------------
// (d) withdraw returns <= requested and the vault delta equals the return
// ---------------------------------------------------------------------------

#[test]
fn partial_withdraw_books_the_delivered_delta() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    fund(&env, &fixture, &ctx, 10_0000000);
    let position = adapter.position();

    // Request twice the deliverable collateral: adapter returns <= requested.
    // The `Allocate` receipt carries the new external-asset balance; the
    // delivered delta is measured on the vault's token balance (the kernel
    // asserts realized == observed delta internally).
    let request = position.c * 2;
    let balance_before = vault_balance(&env, &fixture, &ctx.vault);
    let new_external = allocate(&env, &ctx, request, false);
    let balance_after = vault_balance(&env, &fixture, &ctx.vault);
    let delivered = balance_after - balance_before;

    assert!(delivered > 0, "adapter delivers positive value when C > 0");
    assert!(
        delivered <= request,
        "delivered {delivered} <= requested {request}"
    );
    assert_eq!(
        new_external,
        10_0000000 - delivered,
        "external booked exactly by the delivered delta"
    );
    assert_eq!(adapter.position().c, position.c - delivered);
    let (_, _, external, total_assets) = vault_kernel_state(&env, &ctx.vault);
    assert_eq!(
        external,
        10_0000000 - delivered,
        "external assets decrease exactly by delivered"
    );
    assert_eq!(
        total_assets, 10_0000000,
        "delivered value becomes idle on the vault: total assets unchanged"
    );
    assert_eq!(refresh(&env, &ctx), 10_0000000 - delivered);
}

// ---------------------------------------------------------------------------
// (e) C > principal without refresh: withdraw > principal underflows
// ---------------------------------------------------------------------------

#[test]
fn unrefreshed_extra_collateral_underflows_principal() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    fund(&env, &fixture, &ctx, 10_0000000);

    // Unsolicited donation: 1 idle c unit lands in the adapter, vault not
    // refreshed. Idle-first delivery means the withdrawal consumes it first.
    CarryFixture::mint(&env, &fixture.collateral_token, &ctx.adapter, 1);

    // amount > principal: realized (C = principal + 1) exceeds principal ->
    // `principal underflow on withdraw`.
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_allocate(&env, &ctx, 10_0000001, false)
        .expect_err("withdraw beyond stored principal must underflow");
    assert_eq!(err, ContractError::InvalidState);
    assert_eq!(
        snapshot(&env, &fixture, &ctx),
        before,
        "host frame rolled back"
    );

    // The donation stays in the adapter until the vault refreshes; the next
    // withdrawal at the refreshed principal succeeds.
    let nav = refresh(&env, &ctx);
    assert_eq!(nav, 10_0000001);
    allocate(&env, &ctx, 10_0000001, false);
    assert_eq!(refresh(&env, &ctx), 0);
}

// ---------------------------------------------------------------------------
// (f) liquidation fill + realisation-pending window
// ---------------------------------------------------------------------------

#[test]
fn liquidation_style_loss_refresh_books_jump_and_window_gates() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    // Zero band so the value change reaches NAV immediately.
    let ctx = setup(&env, &fixture, DEFAULTS);
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    fund(&env, &fixture, &ctx, 10_0000000);
    let nav0 = refresh(&env, &ctx);
    let (shares0, price0) = vault_price(&env, &ctx.vault);
    assert_eq!(price0, nav0, "price == NAV at 1 share");

    // Drive the genuine liquidation path: drop the feed so the position is
    // liquidatable, open an auction on the adapter, and let a third-party
    // filler fill it. The fill transfers the debt position with the lot
    // (collateral seized); the yield leg lives on the separate yield pool
    // and stays intact.
    CarryFixture::mint(
        &env,
        &fixture.borrow_token,
        &fixture.third_party,
        10_000_000_000_000,
    );
    fixture.third_party_supply(
        &fixture.pool,
        &fixture.third_party,
        &fixture.borrow_token,
        10_000_000_000_000,
        true,
    );
    fixture.set_oracle_price(&fixture.collateral_token, 40_000_000_000);
    let borrow_assets = soroban_sdk::Vec::from_slice(&env, &[fixture.borrow_token.clone()]);
    let collateral_assets = soroban_sdk::Vec::from_slice(&env, &[fixture.collateral_token.clone()]);
    fixture.pool.new_auction(
        &0u32,
        &ctx.adapter,
        &borrow_assets,
        &collateral_assets,
        &100u32,
    );
    // The auction starts on block `sequence + 1`; fill it 200 blocks in,
    // where the lot scales to 100% and the bid is still undiluted.
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 201);
    // Fill 95% of the auction. A 100% fill of this underwater position
    // would leave bad debt the pool cannot absorb (no backstop), so the
    // pool rejects it; a partial fill transfers the seizure honestly and
    // leaves a residual position.
    let requests = soroban_sdk::Vec::from_slice(
        &env,
        &[templar_soroban_carry_adapter::testutils::Request {
            request_type: 6, // FillUserLiquidationAuction
            address: ctx.adapter.clone(),
            amount: 95,
        }],
    );
    fixture.pool.submit(
        &fixture.third_party,
        &fixture.third_party,
        &fixture.third_party,
        &requests,
    );

    // The fill seized 95% of the collateral and moved 95% of the debt to
    // the filler; the yield leg lives on the separate yield pool and stays
    // intact. The leftover position is deleveraged: `deleverage` deletes
    // the leftover auction and repays the residual debt from the yield leg
    // without touching the collateral.
    let pos = adapter.position();
    assert!(pos.c_pool > 0, "collateral partially seized");
    assert!(pos.d > 0, "residual debt after the partial fill");
    let c_seized = pos.c_pool;
    adapter.deleverage(&ctx.fixture_admin);
    let pos = adapter.position();
    assert_eq!(pos.d, 0, "debt repaid from the yield leg");
    assert_eq!(
        pos.c_pool, c_seized,
        "deleverage leaves collateral untouched"
    );

    // Refresh: NAV reflects the Y-D jump (D -> 0); NAV is collateral-
    // denominated, so the yield leg converts into collateral tokens at the
    // crashed price and the vault's NAV is NOT wiped with the collateral.
    let nav = refresh(&env, &ctx);
    assert_eq!(nav, adapter.total_assets(&fixture.collateral_token));
    assert!(nav > 0, "yield leg preserved through the fill");
    assert!(
        nav != nav0,
        "the seized leg is visible in NAV: {nav} vs {nav0}"
    );

    // Sweep the surplus to the converter; the pending window is fail-closed.
    let surplus = adapter.position().y;
    assert!(surplus > 0, "yield leg intact after the fill");
    adapter.sweep_surplus(&ctx.fixture_admin);
    assert!(adapter.realisation_pending());

    // INSIDE the window: allocate_supply and refresh_markets([this]) fail.
    // The vault kernel transfers the collateral to the adapter *before*
    // invoking `supply`, so the attempt needs vault-custodied collateral to
    // reach the adapter's pending check. Collateral donated straight to the
    // vault stays the vault's own idle balance: the kernel never touches
    // the adapter for NAV-unchanged bookkeeping.
    TokenClient::new(&env, &fixture.collateral_token).transfer(&ctx.user, &ctx.vault, &UNIT);
    let before = snapshot(&env, &fixture, &ctx);
    let err = try_allocate(&env, &ctx, UNIT, true)
        .expect_err("supply must fail while realisation pending");
    // The vault kernel surfaces the adapter's RealisationPending host code
    // (#11, VaultNotIdle by code) verbatim; plan A6(f)'s observable is the
    // failure plus the unchanged state asserted here.
    assert_eq!(err, ContractError::VaultNotIdle);
    assert_eq!(snapshot(&env, &fixture, &ctx), before);

    let err = try_refresh(&env, &ctx).expect_err("refresh must fail while realisation pending");
    assert_eq!(err, ContractError::VaultNotIdle);
    assert_eq!(snapshot(&env, &fixture, &ctx), before);
    // The failed supply/refresh frames rolled back. The donated collateral
    // sits in the vault's idle balance and the kernel values idle at par,
    // so NAV is now nav + UNIT; use that as the in-window baseline.
    assert_eq!(
        vault_balance(&env, &fixture, &ctx.vault),
        UNIT,
        "failed supply/refresh leave the vault's collateral untouched"
    );
    let (shares_b, nav_b) = vault_price(&env, &ctx.vault);
    assert_eq!(shares_b, shares0, "donation never mints shares");
    assert!(
        nav_b >= nav + UNIT - 1 && nav_b <= nav + UNIT,
        "donated collateral is valued at par: {nav_b} vs {nav}"
    );

    // Deposits and atomic redeems still run (the vault kernel never touches
    // the adapter for these). NAV moves by exactly the assets in/out on top
    // of the donated baseline; the adapter and the pools stay untouched.
    deposit(&env, &ctx, UNIT);
    let (shares1, nav1) = vault_price(&env, &ctx.vault);
    assert!(
        shares1 > shares0,
        "deposit still mints shares in the window"
    );
    assert!(
        nav1 >= nav_b + UNIT - 1 && nav1 <= nav_b + UNIT,
        "deposit adds exactly its assets to NAV: {nav1} vs {nav_b}"
    );
    // Redeem of a slice: paid from the vault's own collateral at the stored
    // rate; the pending window must not block or re-price it.
    let receipt = execute(
        &env,
        &ctx.vault,
        &VaultCommand::AtomicRedeem {
            owner: wire(&ctx.user),
            receiver: wire(&ctx.user),
            operator: wire(&ctx.user),
            shares: UNIT,
            min_assets_out: 0,
        },
    );
    let redeemed = decode_i128(&receipt);
    assert!(redeemed > 0, "redeem pays out inside the window");
    assert_eq!(
        vault_balance(&env, &fixture, &ctx.vault),
        2 * UNIT - redeemed,
        "redeem pays strictly from the vault's collateral"
    );
    assert_eq!(
        (
            before.pool_supply.clone(),
            before.pool_liabilities.clone(),
            before.yield_supply.clone()
        ),
        (
            snapshot(&env, &fixture, &ctx).pool_supply,
            snapshot(&env, &fixture, &ctx).pool_liabilities,
            snapshot(&env, &fixture, &ctx).yield_supply
        ),
        "deposits and redeems never touch the adapter during the window"
    );
    // NAV moved down by exactly the paid-out slice.
    let (shares3, nav3) = vault_price(&env, &ctx.vault);
    assert!(
        nav3 <= nav1 && nav3 >= nav1 - redeemed - 1,
        "redeem pays at the stored rate: {nav3} vs {nav1} - {redeemed}"
    );
    let _ = shares3;

    // Converter donates c; admin clears; refresh republishes. The donation
    // replaces the swept value at the 5% surplus haircut, so NAV returns to
    // (within rounding) the level booked before the sweep.
    donate_converted_c(&env, &fixture, &ctx, surplus, 4_000);
    adapter.clear_realisation_pending(&ctx.fixture_admin);
    let nav_after = refresh(&env, &ctx);
    assert!(
        nav_after >= nav,
        "donated conversion restores the swept value: {nav_after} vs {nav}"
    );

    // Per-share value never decreases beyond the booked fill loss: the
    // sweep window must not book an additional dip.
    let (shares2, price2) = vault_price(&env, &ctx.vault);
    assert!(
        price2 >= nav,
        "per-share value restored by the donation: {price2} vs {nav}"
    );
    let _ = shares2;
}

#[test]
fn deleverage_leaves_collateral_and_vault_untouched() {
    let env = Env::default();
    env.mock_all_auths();
    let fixture = CarryFixture::new(&env);
    let ctx = setup(&env, &fixture, DEFAULTS);
    let adapter = CarryAdapterContractClient::new(&env, &ctx.adapter);
    fund(&env, &fixture, &ctx, 10_0000000);
    let position = adapter.position();
    assert!(position.d > 0, "carry position opened on supply");

    adapter.deleverage(&ctx.fixture_admin);
    let pos = adapter.position();
    assert_eq!(pos.d, 0, "debt repaid from the yield leg");
    assert_eq!(pos.c_pool, position.c_pool, "collateral untouched");
    assert_eq!(
        vault_balance(&env, &fixture, &ctx.vault),
        0,
        "vault assets unmoved"
    );
    let pool_positions = fixture.pool.get_positions(&ctx.adapter);
    assert!(
        positions_map(&pool_positions.collateral)
            .iter()
            .any(|(_, v)| *v > 0),
        "adapter keeps its collateral position after deleverage"
    );

    // Vault unharmed: refresh + supply still work after deleverage.
    refresh(&env, &ctx);
    deposit(&env, &ctx, UNIT);
    allocate(&env, &ctx, UNIT, true);
    assert_eq!(
        adapter.position().c,
        position.c + UNIT,
        "supplied collateral joined the position"
    );
}
