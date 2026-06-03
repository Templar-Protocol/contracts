#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Blend interop — proves the SEP-40 adapter satisfies the price-feed
//! interface a real Blend pool consumes.
//!
//! `BlendFixture::deploy` instantiates the full Blend stack (backstop,
//! emitter, pool factory) from the canonical WASM artifacts shipped in
//! `blend-contract-sdk`; the pool factory then deploys a pool pointing at
//! our SEP-40 adapter as its oracle. From that point on, anything Blend's
//! pool does that touches the oracle goes through `Sep40Adapter ::
//! PriceFeedTrait`, which delegates to the proxy-oracle runtime.
//!
//! Coverage:
//! - Blend-1: pool deploys and reserve setup succeeds with our adapter as oracle.
//! - Blend-2: tripping a circuit breaker on our runtime causes `lastprice`
//!   to return None — the SEP-40 contract any Blend pool relies on for
//!   freshness signaling.
//! - Blend-3: advancing past `max_age_secs` does the same.

use blend_contract_sdk::pool;
use blend_contract_sdk::testutils::{default_reserve_config, BlendFixture};
use soroban_sdk::testutils::{Address as _, BytesN as _, Ledger as _, LedgerInfo};
use soroban_sdk::{token::StellarAssetClient, Address, BytesN, Env, Symbol, Vec as SVec};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, ProxyConfig, SorobanDecimal, SourceConfig, StepwiseChangeConfig,
};
use templar_proxy_oracle_soroban_contract::{SorobanProxyOracle, SorobanProxyOracleClient};
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_governance_contract::{
    ProxyOracleGovernance, ProxyOracleGovernanceClient,
};
use templar_proxy_oracle_soroban_integration_tests::common::{
    ledger, MockOracle, MockOracleClient,
};
use templar_proxy_oracle_soroban_sep40_adapter_contract::{Sep40Adapter, Sep40AdapterClient};

/// Variant of `Bootstrap` that uses a Stellar Asset Contract for the tracked
/// asset (Blend tracks reserves by SAC address, so the adapter and runtime
/// must speak `Asset::Stellar(sac_addr)`).
struct BlendBootstrap {
    env: Env,
    admin: Address,
    deployer: Address,
    asset_sac: Address,
    asset_admin: StellarAssetClient<'static>,
    btc_asset: Asset,
    base_usd: Asset,
    runtime: SorobanProxyOracleClient<'static>,
    governance: ProxyOracleGovernanceClient<'static>,
    adapter_id: Address,
    adapter: Sep40AdapterClient<'static>,
    upstream: MockOracleClient<'static>,
    pool_addr: Address,
    pool: pool::Client<'static>,
}

fn setup_blend_bootstrap() -> BlendBootstrap {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 4096,
        max_entry_ttl: 6_312_000,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let deployer = Address::generate(&env);
    let base_usd = Asset::Other(Symbol::new(&env, "USD"));

    // Stellar Asset Contract for the asset Blend will track.
    let asset_sac_client = env.register_stellar_asset_contract_v2(deployer.clone());
    let asset_sac = asset_sac_client.address();
    let asset_admin = StellarAssetClient::new(&env, &asset_sac);
    let btc_asset = Asset::Stellar(asset_sac.clone());

    // Runtime + governance.
    let runtime_id = env.register(SorobanProxyOracle, (&admin, &base_usd));
    let runtime = SorobanProxyOracleClient::new(&env, &runtime_id);
    let governance_id = env.register(ProxyOracleGovernance, (&admin, &runtime_id, 0_u64));
    let governance = ProxyOracleGovernanceClient::new(&env, &governance_id);
    let live_until_ledger = env.ledger().max_live_until_ledger();
    runtime.transfer_ownership(&governance_id, &live_until_ledger);
    runtime.accept_ownership();

    // Mock upstream source (SEP-40 PriceFeedTrait).
    let upstream_id = env.register(MockOracle, (&base_usd, &7_u32, &1_u32));
    let upstream = MockOracleClient::new(&env, &upstream_id);

    // Adapter, owned by admin, pointing at the runtime, asset = the SAC.
    let adapter_id = env.register(
        Sep40Adapter,
        (&admin, &runtime_id, &btc_asset, &7_u32, &1_u32, &base_usd),
    );
    let adapter = Sep40AdapterClient::new(&env, &adapter_id);

    // Configure the runtime feed.
    let mut sources = SVec::new(&env);
    sources.push_back(SourceConfig {
        oracle: upstream_id,
        asset: btc_asset.clone(),
    });
    let create_id = governance.submit(
        &admin,
        &GovernanceAction::SetProxy(
            btc_asset.clone(),
            ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );
    governance.accept(&admin, &create_id);

    // Blend stack + pool pointing at our adapter as oracle.
    let blnd = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let usdc = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let blend = BlendFixture::deploy(&env, &deployer, &blnd, &usdc);

    let pool_addr = blend.pool_factory.mock_all_auths().deploy(
        &deployer,
        &soroban_sdk::String::from_str(&env, "templar-proxy-oracle-pool"),
        &BytesN::<32>::random(&env),
        &adapter_id,
        &1_000_000_u32,
        &4_u32,
        &1_0000000_i128,
    );
    let pool = pool::Client::new(&env, &pool_addr);
    let reserve_config = default_reserve_config();
    pool.mock_all_auths()
        .queue_set_reserve(&asset_sac, &reserve_config);
    pool.mock_all_auths().set_reserve(&asset_sac);
    blend
        .backstop
        .mock_all_auths()
        .deposit(&deployer, &pool_addr, &500_000_000_000_i128);
    pool.mock_all_auths().set_status(&3_u32);
    pool.mock_all_auths().update_status();

    BlendBootstrap {
        env,
        admin,
        deployer,
        asset_sac,
        asset_admin,
        btc_asset,
        base_usd,
        runtime,
        governance,
        adapter_id,
        adapter,
        upstream,
        pool_addr,
        pool,
    }
}

#[test]
fn blend_pool_deploys_with_proxy_oracle_adapter_as_oracle() {
    // Setup-only sanity test: confirms the SEP-40 interface our adapter
    // implements is wire-compatible with what the Blend pool factory + pool
    // expect during deploy and reserve configuration. If the adapter's ABI
    // ever diverged from SEP-40, this test would fail at deploy.
    let b = setup_blend_bootstrap();
    assert_ne!(b.pool_addr, Address::generate(&b.env));
    assert_eq!(b.pool.get_config().oracle, b.adapter_id);
}

#[test]
fn blend_oracle_sees_accepted_price_with_correct_decimals() {
    let b = setup_blend_bootstrap();
    // Push a healthy price to the upstream and refresh; the adapter should
    // then report a SEP-40 price at the adapter's decimals (7 in this setup).
    b.upstream
        .set_price(&b.btc_asset, &1_000_000_000_i128, &100_u64);
    let assets = SVec::from_array(&b.env, [b.btc_asset.clone()]);
    let _ = b.runtime.refresh(&assets);

    let sep40 = b.adapter.lastprice(&b.btc_asset).unwrap();
    // 1_000_000_000 (i.e. 100.0000000 at 7 decimals) round-trips since
    // adapter_decimals==source_decimals.
    assert_eq!(sep40.price, 1_000_000_000);
    assert_eq!(sep40.timestamp, 100);
}

#[test]
fn blend_oracle_sees_none_when_circuit_breaker_trips() {
    let b = setup_blend_bootstrap();
    // Install a tight stepwise breaker and force a trip on the second refresh.
    b.governance.submit(
        &b.admin,
        &GovernanceAction::ConfigureBreakers(b.btc_asset.clone(), 0, 8),
    );
    let id = b.governance.next_proposal_id() - 1;
    b.governance.accept(&b.admin, &id);
    b.governance.submit(
        &b.admin,
        &GovernanceAction::AddBreaker(
            b.btc_asset.clone(),
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF),
            }),
        ),
    );
    let id = b.governance.next_proposal_id() - 1;
    b.governance.accept(&b.admin, &id);

    let assets = SVec::from_array(&b.env, [b.btc_asset.clone()]);
    b.upstream
        .set_price(&b.btc_asset, &1_000_000_000_i128, &100_u64);
    let _ = b.runtime.refresh(&assets);
    ledger::advance_secs(&b.env, 1);
    b.upstream
        .set_price(&b.btc_asset, &3_000_000_000_i128, &101_u64);
    let _ = b.runtime.refresh(&assets);

    // From the Blend pool's perspective, the oracle has nothing to report.
    assert!(b.adapter.lastprice(&b.btc_asset).is_none());
}

#[test]
fn blend_oracle_sees_none_when_freshness_window_expires() {
    let b = setup_blend_bootstrap();
    let assets = SVec::from_array(&b.env, [b.btc_asset.clone()]);
    b.upstream
        .set_price(&b.btc_asset, &1_000_000_000_i128, &100_u64);
    let _ = b.runtime.refresh(&assets);
    assert!(b.adapter.lastprice(&b.btc_asset).is_some());

    // Advance past max_age_secs=300.
    ledger::advance_secs(&b.env, 400);
    assert!(b.adapter.lastprice(&b.btc_asset).is_none());

    // Suppress unused-field warnings.
    let _ = (&b.deployer, &b.base_usd, &b.asset_admin, &b.asset_sac);
}
