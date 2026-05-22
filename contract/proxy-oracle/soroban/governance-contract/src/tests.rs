use super::*;

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, CircuitBreakerUpdateConfig, MonotonicRunConfig, RearmConfig,
    SetEnforcedConfig, SourceConfig, StepwiseChangeConfig, WindowedChangeDeltaConfig,
};
use templar_proxy_oracle_soroban_contract::{SorobanProxyOracle, SorobanProxyOracleClient};

fn setup() -> (
    Env,
    Address,
    Address,
    Address,
    SorobanProxyOracleClient<'static>,
) {
    setup_with_ttl(10_000_000_000)
}

fn setup_with_ttl(
    action_ttl_ns: u64,
) -> (
    Env,
    Address,
    Address,
    Address,
    SorobanProxyOracleClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });

    let admin = Address::generate(&env);
    let base = Asset::Other(Symbol::new(&env, "USD"));
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base, 8_u32, 1_u32));
    let governance_id = env.register(ProxyOracleGovernance, (&admin, &proxy_id, action_ttl_ns));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    proxy.set_governance(&governance_id);

    (env, admin, proxy_id, governance_id, proxy)
}

fn decimal_repr(env: &Env, value: Decimal) -> Vec<u64> {
    Vec::from_array(env, value.as_repr())
}

fn sample_proxy_config(env: &Env, asset: Asset, source: Address) -> ProxyConfig {
    let mut sources = Vec::new(env);
    sources.push_back(SourceConfig {
        oracle: source,
        asset,
    });
    ProxyConfig {
        sources,
        min_sources: 1,
        max_age_secs: Some(30),
        max_clock_drift_secs: Some(5),
    }
}

fn accept_now(env: &Env, governance_id: &Address, admin: &Address, proposal_id: u64) {
    env.as_contract(governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), proposal_id).unwrap();
    });
}

fn submit_now(
    env: &Env,
    governance_id: &Address,
    admin: &Address,
    action: GovernanceAction,
) -> u64 {
    env.as_contract(governance_id, || {
        ProxyOracleGovernance::submit(env.clone(), admin.clone(), action).unwrap()
    })
}

#[test]
fn accept_requires_action_ttl_to_elapse() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup();
    let next_governance = Address::generate(&env);

    let proposal_id = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetGovernance(next_governance.clone()),
    );

    let pending = env
        .as_contract(&governance_id, || {
            ProxyOracleGovernance::pending(env.clone(), proposal_id)
        })
        .unwrap();
    assert_eq!(pending.valid_after_ns, 110_000_000_000);

    let early = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), proposal_id)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));
    assert_eq!(proxy.governance(), Some(governance_id.clone()));

    env.ledger().set(LedgerInfo {
        timestamp: 110,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), proposal_id).unwrap();
    });

    assert_eq!(proxy.governance(), Some(next_governance));
}

#[test]
fn breaker_governance_workflows_execute_through_runtime() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup_with_ttl(0);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let source = Address::generate(&env);

    let set_proxy = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset.clone(),
            sample_proxy_config(&env, asset.clone(), source),
        ),
    );
    accept_now(&env, &governance_id, &admin, set_proxy);
    assert!(proxy.get_proxy(&asset).is_some());

    let configure = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::ConfigureBreakers(asset.clone(), 0, 8),
    );
    accept_now(&env, &governance_id, &admin, configure);

    let stepwise = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::AddBreaker(
            asset.clone(),
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change_repr: decimal_repr(&env, Decimal::ONE_HALF),
            }),
        ),
    );
    accept_now(&env, &governance_id, &admin, stepwise);

    let unenforce = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::UpdateBreaker(
            asset.clone(),
            0,
            CircuitBreakerUpdateConfig::SetEnforced(SetEnforcedConfig { is_enforced: false }),
        ),
    );
    accept_now(&env, &governance_id, &admin, unenforce);

    let rearm = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::UpdateBreaker(
            asset.clone(),
            0,
            CircuitBreakerUpdateConfig::Rearm(RearmConfig {
                armed_after_secs: 100,
                accepted_history_source_code: 0,
            }),
        ),
    );
    accept_now(&env, &governance_id, &admin, rearm);

    let remove = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::RemoveBreaker(asset.clone(), 0),
    );
    accept_now(&env, &governance_id, &admin, remove);
    assert_eq!(proxy.get_breaker_set_view(&asset).unwrap().breaker_count, 0);

    let monotonic = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::AddBreaker(
            asset.clone(),
            CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
                max_streak: 3,
                min_relative_step_change_repr: decimal_repr(&env, Decimal::ONE_HALF),
            }),
        ),
    );
    accept_now(&env, &governance_id, &admin, monotonic);

    let windowed = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::AddBreaker(
            asset.clone(),
            CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
                window_len: 2,
                lookback_windows: 1,
                max_relative_change_delta_repr: decimal_repr(&env, Decimal::ONE_HALF),
            }),
        ),
    );
    accept_now(&env, &governance_id, &admin, windowed);
    let view = proxy.get_breaker_set_view(&asset).unwrap();
    assert_eq!(view.breaker_count, 2);
    assert_eq!(view.next_id, 3);
}

#[test]
fn remove_proxy_and_set_action_ttl_execute_through_governance() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup_with_ttl(0);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let source = Address::generate(&env);

    let set_proxy = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset.clone(),
            sample_proxy_config(&env, asset.clone(), source),
        ),
    );
    accept_now(&env, &governance_id, &admin, set_proxy);
    assert!(proxy.get_proxy(&asset).is_some());

    let remove_proxy = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::RemoveProxy(asset.clone()),
    );
    accept_now(&env, &governance_id, &admin, remove_proxy);
    assert!(proxy.get_proxy(&asset).is_none());

    let set_ttl = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(42),
    );
    accept_now(&env, &governance_id, &admin, set_ttl);
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::action_ttl_ns(env.clone())
        }),
        42
    );
}

#[test]
fn accept_requires_lowest_pending_proposal_id() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup_with_ttl(0);
    let asset_one = Asset::Other(Symbol::new(&env, "BTC"));
    let asset_two = Asset::Other(Symbol::new(&env, "ETH"));
    let source = Address::generate(&env);

    let first = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset_one.clone(),
            sample_proxy_config(&env, asset_one.clone(), source.clone()),
        ),
    );
    let second = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset_two.clone(),
            sample_proxy_config(&env, asset_two.clone(), source),
        ),
    );

    let out_of_order = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), second)
    });

    assert_eq!(out_of_order, Err(GovernanceError::ProposalOutOfOrder));
    assert!(proxy.get_proxy(&asset_two).is_none());
    accept_now(&env, &governance_id, &admin, first);
    accept_now(&env, &governance_id, &admin, second);
    assert!(proxy.get_proxy(&asset_one).is_some());
    assert!(proxy.get_proxy(&asset_two).is_some());
}

#[test]
fn revoke_unblocks_later_proposal() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup_with_ttl(0);
    let asset_one = Asset::Other(Symbol::new(&env, "BTC"));
    let asset_two = Asset::Other(Symbol::new(&env, "ETH"));
    let source = Address::generate(&env);

    let first = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset_one.clone(),
            sample_proxy_config(&env, asset_one.clone(), source.clone()),
        ),
    );
    let second = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(
            asset_two.clone(),
            sample_proxy_config(&env, asset_two.clone(), source),
        ),
    );

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::revoke(env.clone(), admin.clone(), first).unwrap();
    });
    accept_now(&env, &governance_id, &admin, second);

    assert!(proxy.get_proxy(&asset_one).is_none());
    assert!(proxy.get_proxy(&asset_two).is_some());
}

#[test]
fn submit_requires_admin_auth() {
    let env = Env::default();
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 100,
        ..Default::default()
    });
    let admin = Address::generate(&env);
    let proxy = Address::generate(&env);
    let governance_id = env.register(ProxyOracleGovernance, (&admin, &proxy, 0_u64));
    let client = ProxyOracleGovernanceClient::new(&env, &governance_id);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));

    let result = client.try_submit(&admin, &GovernanceAction::RemoveProxy(asset));

    assert!(result.is_err());
}

#[test]
fn same_asset_proxy_proposals_can_coexist_and_execute_in_order() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup();
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let source_asset = asset.clone();
    let source_one = Address::generate(&env);
    let source_two = Address::generate(&env);

    let config = |source: Address| {
        let mut sources = Vec::new(&env);
        sources.push_back(SourceConfig {
            oracle: source,
            asset: source_asset.clone(),
        });
        ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(30),
            max_clock_drift_secs: Some(5),
        }
    };

    let first = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(asset.clone(), config(source_one.clone())),
    );
    let second = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetProxy(asset.clone(), config(source_two.clone())),
    );
    assert_ne!(first, second);

    let ids = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::pending_ids(env.clone())
    });
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.get(0).unwrap(), first);
    assert_eq!(ids.get(1).unwrap(), second);

    env.ledger().set(LedgerInfo {
        timestamp: 111,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), first).unwrap();
    });
    assert_eq!(
        proxy
            .get_proxy(&asset)
            .unwrap()
            .sources
            .get(0)
            .unwrap()
            .oracle,
        source_one
    );
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::accept(env.clone(), admin.clone(), second).unwrap();
    });

    assert_eq!(proxy.get_proxy(&asset).unwrap().sources.len(), 1);
    assert_eq!(
        proxy
            .get_proxy(&asset)
            .unwrap()
            .sources
            .get(0)
            .unwrap()
            .oracle,
        source_two
    );
}
