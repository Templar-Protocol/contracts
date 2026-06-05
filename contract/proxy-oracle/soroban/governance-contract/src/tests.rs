use super::*;

use alloc::vec;
use alloc::vec::Vec as StdVec;
use soroban_sdk::testutils::{
    storage::Persistent as _, Address as _, Events as _, Ledger, LedgerInfo,
};
use soroban_sdk::{Bytes, BytesN, Event, Symbol};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, MonotonicRunConfig, ProxyConfig, RearmConfig, SetEnforcedConfig,
    SorobanDecimal, SourceConfig, StepwiseChangeConfig, WindowedChangeDeltaConfig,
};
use templar_proxy_oracle_soroban_contract::{SorobanProxyOracle, SorobanProxyOracleClient};
use templar_proxy_oracle_soroban_governance_common::{OperationKind, Role, MAX_PROPOSAL_TTL_NS};

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
    let proxy_id = env.register(SorobanProxyOracle, (&admin, &base));
    let governance_id = env.register(ProxyOracleGovernance, (&admin, &proxy_id, action_ttl_ns));
    let proxy = SorobanProxyOracleClient::new(&env, &proxy_id);
    // The runtime contract delegates ownership to `stellar_access::ownable`'s
    // two-step transfer. We initiate transfer as the initial owner and the
    // governance contract accepts. `mock_all_auths` covers both auth checks.
    let live_until_ledger = env.ledger().max_live_until_ledger();
    proxy.transfer_ownership(&governance_id, &live_until_ledger);
    proxy.accept_ownership();

    (env, admin, proxy_id, governance_id, proxy)
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
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), proposal_id).unwrap();
    });
}

fn submit_now(
    env: &Env,
    governance_id: &Address,
    admin: &Address,
    action: GovernanceAction,
) -> u64 {
    env.as_contract(governance_id, || {
        let id = ProxyOracleGovernance::next_proposal_id(env.clone()).unwrap();
        ProxyOracleGovernance::create_proposal(env.clone(), admin.clone(), id, action, 0).unwrap();
        id
    })
}

fn governance_events(
    env: &Env,
    governance_id: &Address,
) -> StdVec<soroban_sdk::xdr::ContractEvent> {
    env.events()
        .all()
        .filter_by_contract(governance_id)
        .events()
        .to_vec()
}

#[test]
fn event_submit_accept_revoke_handoff_and_ttl_topics_payloads_are_exact() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let next_governance = Address::generate(&env);

    let handoff = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::TransferOwnership(next_governance.clone()),
    );
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![
            ProposalSubmitted {
                id: handoff,
                valid_after_ns: 100_000_000_000,
                action_code: 9,
            }
            .to_xdr(&env, &governance_id),
            OwnershipTransferSubmitted {
                id: handoff,
                new_owner: next_governance,
            }
            .to_xdr(&env, &governance_id),
        ]
    );

    accept_now(&env, &governance_id, &admin, handoff);
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![ProposalAccepted { id: handoff }.to_xdr(&env, &governance_id)]
    );

    let ttl = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 42),
    );
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![ProposalSubmitted {
            id: ttl,
            valid_after_ns: 100_000_000_000,
            action_code: 10,
        }
        .to_xdr(&env, &governance_id)]
    );

    accept_now(&env, &governance_id, &admin, ttl);
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![
            ActionTtlSet {
                kind: OperationKind::SetProxy,
                new_ttl_ns: 42
            }
            .to_xdr(&env, &governance_id),
            ProposalAccepted { id: ttl }.to_xdr(&env, &governance_id),
        ]
    );

    let revoke = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::RemoveProxy(Asset::Other(Symbol::new(&env, "BTC"))),
    );
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![ProposalSubmitted {
            id: revoke,
            valid_after_ns: 100_000_000_000,
            action_code: 2,
        }
        .to_xdr(&env, &governance_id)]
    );

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::cancel_proposal(env.clone(), admin.clone(), revoke).unwrap();
    });
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![ProposalRevoked { id: revoke }.to_xdr(&env, &governance_id)]
    );

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::extend_ttl(env.clone()).unwrap();
    });
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![TtlExtended {}.to_xdr(&env, &governance_id)]
    );
}

#[test]
fn event_failed_accept_does_not_emit_false_accepted_event() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup();
    let proposal_id = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 42),
    );

    let early = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), proposal_id)
    });

    assert_eq!(early, Err(GovernanceError::ProposalNotMature));
    assert_eq!(governance_events(&env, &governance_id), vec![]);
}

#[test]
fn parity_governance_allows_out_of_order_execution() {
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

    let ids = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::active_ids(env.clone())
    });
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.get(0).unwrap(), first);
    assert_eq!(ids.get(1).unwrap(), second);

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), second).unwrap();
    });
    assert!(proxy.get_proxy(&asset_one).is_none());
    assert!(proxy.get_proxy(&asset_two).is_some());

    accept_now(&env, &governance_id, &admin, first);
    assert!(proxy.get_proxy(&asset_one).is_some());
    assert!(proxy.get_proxy(&asset_two).is_some());
}

#[test]
fn parity_governance_revoke_unblocks_later_ordered_proposal() {
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
        ProxyOracleGovernance::cancel_proposal(env.clone(), admin.clone(), first).unwrap();
    });
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![ProposalRevoked { id: first }.to_xdr(&env, &governance_id)]
    );

    accept_now(&env, &governance_id, &admin, second);
    assert!(proxy.get_proxy(&asset_one).is_none());
    assert!(proxy.get_proxy(&asset_two).is_some());
}

#[test]
fn accept_requires_action_ttl_to_elapse() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup();
    let next_governance = Address::generate(&env);

    let proposal_id = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::TransferOwnership(next_governance.clone()),
    );

    let proposal = env
        .as_contract(&governance_id, || {
            ProxyOracleGovernance::get_proposal(env.clone(), proposal_id)
        })
        .unwrap();
    assert_eq!(proposal.created_at_ns + proposal.ttl_ns, 110_000_000_000);

    let early = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), proposal_id)
    });
    assert_eq!(early, Err(GovernanceError::ProposalNotMature));
    assert_eq!(proxy.get_owner(), Some(governance_id.clone()));

    env.ledger().set(LedgerInfo {
        timestamp: 110,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), proposal_id).unwrap();
    });

    // The proposal initiated `transfer_ownership`; the new owner must
    // explicitly accept to finalize the handoff under the two-step
    // ownership pattern provided by `stellar_access::ownable`.
    assert_eq!(proxy.get_owner(), Some(governance_id.clone()));
    proxy.accept_ownership();
    assert_eq!(proxy.get_owner(), Some(next_governance));
}

#[test]
fn accept_ownership_proposal_finalizes_handoff_to_a_new_governance_contract() {
    // Two governance contracts back-to-back: the second one (`next_gov`) is
    // the pending owner, and uses its own `AcceptOwnership` proposal to
    // claim ownership without anyone calling `proxy.accept_ownership()`
    // directly. This exercises the dispatch path added for the second leg
    // of `stellar_access::ownable`'s two-step transfer.
    let (env, admin, proxy_id, governance_id, proxy) = setup_with_ttl(0);
    let next_admin = Address::generate(&env);
    let next_gov_id = env.register(ProxyOracleGovernance, (&next_admin, &proxy_id, 0_u64));

    // Old governance hands off to new governance.
    let handoff = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::TransferOwnership(next_gov_id.clone()),
    );
    accept_now(&env, &governance_id, &admin, handoff);
    // Pending transfer: owner is still old governance.
    assert_eq!(proxy.get_owner(), Some(governance_id.clone()));

    // New governance finalizes ownership via its own AcceptOwnership proposal.
    let claim = submit_now(
        &env,
        &next_gov_id,
        &next_admin,
        GovernanceAction::AcceptOwnership,
    );
    accept_now(&env, &next_gov_id, &next_admin, claim);
    assert_eq!(proxy.get_owner(), Some(next_gov_id));
}

#[test]
fn renounce_ownership_proposal_permanently_relinquishes_ownership() {
    let (env, admin, _proxy_id, governance_id, proxy) = setup_with_ttl(0);

    let renounce = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::RenounceOwnership,
    );
    accept_now(&env, &governance_id, &admin, renounce);

    // After renounce, the proxy has no owner. Any subsequent `#[only_owner]`
    // call panics with `OwnableError::OwnerNotSet` (2100) — out of band of
    // the typed `ContractError` enum that the governance dispatch path
    // catches — so mutations are effectively, permanently blocked.
    assert_eq!(proxy.get_owner(), None);
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
                max_relative_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
            }),
        ),
    );
    accept_now(&env, &governance_id, &admin, stepwise);

    let unenforce = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetEnforced(asset.clone(), 0, SetEnforcedConfig { is_enforced: false }),
    );
    accept_now(&env, &governance_id, &admin, unenforce);

    let rearm = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::Rearm(
            asset.clone(),
            0,
            RearmConfig {
                armed_after_secs: 100,
                accepted_history_source_code: 0,
            },
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
                min_relative_step_change: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
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
                max_relative_change_delta: SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF),
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
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 42),
    );
    accept_now(&env, &governance_id, &admin, set_ttl);
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::SetProxy).unwrap()
        }),
        42
    );
}

#[test]
fn accept_allows_any_mature_pending_proposal_id() {
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
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), second).unwrap();
    });

    assert!(proxy.get_proxy(&asset_two).is_some());
    accept_now(&env, &governance_id, &admin, first);
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
        ProxyOracleGovernance::cancel_proposal(env.clone(), admin.clone(), first).unwrap();
    });
    accept_now(&env, &governance_id, &admin, second);

    assert!(proxy.get_proxy(&asset_one).is_none());
    assert!(proxy.get_proxy(&asset_two).is_some());
}

// ── TTL tests ─────────────────────────────────────────────────────────────────

#[test]
fn ttl_governance_extend_is_permissionless_and_emits_event() {
    let (env, _admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::extend_ttl(env.clone()).unwrap();
    });
    assert_eq!(
        governance_events(&env, &governance_id),
        vec![TtlExtended {}.to_xdr(&env, &governance_id)]
    );
}

#[test]
fn ttl_governance_extend_renews_active_proposals() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(10);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let proposal_id = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::RemoveProxy(asset),
    );
    env.ledger().set(LedgerInfo {
        timestamp: 100,
        protocol_version: 25,
        sequence_number: 2_592_100,
        max_entry_ttl: 3_110_400,
        ..Default::default()
    });

    let key = DataKey::Proposal(proposal_id);
    let ttl_before = env.as_contract(&governance_id, || env.storage().persistent().get_ttl(&key));
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::extend_ttl(env.clone()).unwrap();
    });
    let ttl_after = env.as_contract(&governance_id, || env.storage().persistent().get_ttl(&key));

    assert!(ttl_after > ttl_before);
}

// ── missing_config tests ──────────────────────────────────────────────────────

#[test]
fn missing_config_create_proposal_fails_closed_on_missing_governance_state() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));

    env.as_contract(&governance_id, || {
        env.storage().instance().remove(&DataKey::Header);
    });

    let result = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin.clone(),
            0,
            GovernanceAction::RemoveProxy(asset),
            0,
        )
    });
    assert_eq!(result, Err(GovernanceError::MissingConfig));
}

#[test]
fn missing_config_get_operation_ttl_fails_closed_on_missing_governance_state() {
    let (env, _admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);

    env.as_contract(&governance_id, || {
        env.storage().instance().remove(&DataKey::Header);
    });

    let result = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::SetProxy)
    });
    assert_eq!(result, Err(GovernanceError::MissingConfig));
}

#[test]
fn create_proposal_requires_admin_auth() {
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

    let result = client.try_create_proposal(&admin, &0, &GovernanceAction::RemoveProxy(asset), &0);

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
        ProxyOracleGovernance::active_ids(env.clone())
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
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), first).unwrap();
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
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), second).unwrap();
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

#[test]
fn proposal_views_and_per_operation_ttls_match_near_lifecycle() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(10);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let source = Address::generate(&env);
    let set_proxy = GovernanceAction::SetProxy(
        asset.clone(),
        sample_proxy_config(&env, asset.clone(), source),
    );

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::next_proposal_id(env.clone()).unwrap()
        }),
        0
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::SetProxy).unwrap()
        }),
        10
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_effective_proposal_ttl(env.clone(), set_proxy.clone(), 42)
                .unwrap()
        }),
        42
    );

    let proposal = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(env.clone(), admin.clone(), 0, set_proxy.clone(), 42)
            .unwrap()
    });
    assert_eq!(proposal.created_at_ns, 100_000_000_000);
    assert_eq!(proposal.ttl_ns, 42);
    assert_eq!(proposal.created_by, admin);
    assert_eq!(proposal.operation, set_proxy);
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::active_ids(env.clone())
                .iter()
                .collect::<StdVec<_>>()
        }),
        vec![0]
    );
    assert!(env
        .as_contract(&governance_id, || {
            ProxyOracleGovernance::get_proposal(env.clone(), 0)
        })
        .is_some());
}

#[test]
fn role_specific_auth_admin_override_and_targeted_revoke() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let manager = Address::generate(&env);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let source = Address::generate(&env);
    let set_proxy = GovernanceAction::SetProxy(
        asset.clone(),
        sample_proxy_config(&env, asset.clone(), source),
    );

    let unauthorized = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            manager.clone(),
            0,
            set_proxy.clone(),
            0,
        )
    });
    assert_eq!(unauthorized, Err(GovernanceError::Unauthorized));

    let grant_manager = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(manager.clone(), Role::ProxyConfigurationManager, true),
    );
    accept_now(&env, &governance_id, &admin, grant_manager);
    let grant_tripper = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(manager.clone(), Role::ManualTripper, true),
    );
    accept_now(&env, &governance_id, &admin, grant_tripper);

    assert!(env.as_contract(&governance_id, || {
        ProxyOracleGovernance::has_role(
            env.clone(),
            manager.clone(),
            Role::ProxyConfigurationManager,
        )
    }));
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_roles(env.clone(), manager.clone()).len()
        }),
        2
    );

    let manager_created = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(env.clone(), manager.clone(), 2, set_proxy, 0)
            .unwrap()
    });
    assert_eq!(manager_created.created_by, manager);

    let admin_override = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin.clone(),
            3,
            GovernanceAction::SetManualTrip(asset, true, None),
            0,
        )
        .unwrap()
    });
    assert_eq!(admin_override.ttl_ns, 0);

    let revoke_manager = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(manager.clone(), Role::ProxyConfigurationManager, false),
    );
    accept_now(&env, &governance_id, &admin, revoke_manager);
    assert!(!env.as_contract(&governance_id, || {
        ProxyOracleGovernance::has_role(
            env.clone(),
            manager.clone(),
            Role::ProxyConfigurationManager,
        )
    }));
    assert!(env.as_contract(&governance_id, || {
        ProxyOracleGovernance::has_role(env.clone(), manager, Role::ManualTripper)
    }));
}

#[test]
fn last_admin_protection_and_admin_role_views() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let second_admin = Address::generate(&env);

    let remove_last = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(admin.clone(), Role::Admin, false),
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), remove_last)
        }),
        Err(GovernanceError::LastAdmin)
    );

    let grant_second = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(second_admin.clone(), Role::Admin, true),
    );
    accept_now(&env, &governance_id, &admin, grant_second);
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::list_role(env.clone(), Role::Admin).len()
        }),
        2
    );
    accept_now(&env, &governance_id, &admin, remove_last);
    assert!(!env.as_contract(&governance_id, || {
        ProxyOracleGovernance::has_role(env.clone(), admin, Role::Admin)
    }));
    assert!(env.as_contract(&governance_id, || {
        ProxyOracleGovernance::has_role(env.clone(), second_admin, Role::Admin)
    }));
}

#[test]
fn validation_rejects_empty_proxy_invalid_ttls_and_large_metadata() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));
    let empty_proxy = ProxyConfig {
        sources: Vec::new(&env),
        min_sources: 0,
        max_age_secs: None,
        max_clock_drift_secs: None,
    };
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                0,
                GovernanceAction::SetProxy(asset.clone(), empty_proxy),
                0,
            )
        }),
        Err(GovernanceError::InvalidInput)
    );

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                0,
                GovernanceAction::RemoveProxy(asset.clone()),
                MAX_PROPOSAL_TTL_NS + 1,
            )
        }),
        Err(GovernanceError::TtlExceedsMaximum)
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                0,
                GovernanceAction::SetActionTtl(OperationKind::RemoveProxy, MAX_PROPOSAL_TTL_NS + 1),
                0,
            )
        }),
        Err(GovernanceError::TtlExceedsMaximum)
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                0,
                GovernanceAction::SetManualTrip(
                    asset,
                    true,
                    Some(Bytes::from_array(&env, &[7_u8; 1025])),
                ),
                0,
            )
        }),
        Err(GovernanceError::InvalidInput)
    );
}

#[test]
fn set_action_ttl_uses_maximum_of_set_ttl_and_target_ttl() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(10);
    let raise_set_proxy = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 100),
    );
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    accept_now(&env, &governance_id, &admin, raise_set_proxy);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_effective_proposal_ttl(
                env.clone(),
                GovernanceAction::SetActionTtl(OperationKind::SetProxy, 1),
                0,
            )
            .unwrap()
        }),
        100
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_effective_proposal_ttl(
                env.clone(),
                GovernanceAction::SetActionTtl(OperationKind::RemoveProxy, 1),
                0,
            )
            .unwrap()
        }),
        10
    );
}

#[test]
fn breaker_lifecycle_ttls_are_independent() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let asset = Asset::Other(Symbol::new(&env, "BTC"));

    let set_rearm_ttl = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::Rearm, 20),
    );
    accept_now(&env, &governance_id, &admin, set_rearm_ttl);
    let set_enforced_ttl = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::SetEnforced, 30),
    );
    accept_now(&env, &governance_id, &admin, set_enforced_ttl);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::Rearm).unwrap()
        }),
        20
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::SetEnforced)
                .unwrap()
        }),
        30
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_effective_proposal_ttl(
                env.clone(),
                GovernanceAction::Rearm(
                    asset.clone(),
                    0,
                    RearmConfig {
                        armed_after_secs: 0,
                        accepted_history_source_code: 0,
                    },
                ),
                0,
            )
            .unwrap()
        }),
        20
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_effective_proposal_ttl(
                env.clone(),
                GovernanceAction::SetEnforced(asset, 0, SetEnforcedConfig { is_enforced: false }),
                0,
            )
            .unwrap()
        }),
        30
    );
}

#[test]
fn set_action_ttl_requires_proxy_configuration_manager() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let manager = Address::generate(&env);
    let non_manager = Address::generate(&env);

    let grant_manager = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetRole(manager.clone(), Role::ProxyConfigurationManager, true),
    );
    accept_now(&env, &governance_id, &admin, grant_manager);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                non_manager,
                1,
                GovernanceAction::SetActionTtl(OperationKind::Upgrade, 42),
                0,
            )
        }),
        Err(GovernanceError::Unauthorized)
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                manager.clone(),
                1,
                GovernanceAction::SetActionTtl(OperationKind::Upgrade, 42),
                0,
            )
            .unwrap()
            .created_by
        }),
        manager
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                manager.clone(),
                2,
                GovernanceAction::SetActionTtl(OperationKind::SetProxy, 42),
                0,
            )
            .unwrap()
            .created_by
        }),
        manager
    );

    let admin_only_ttl = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin.clone(),
            3,
            GovernanceAction::SetActionTtl(OperationKind::TransferOwnership, 42),
            0,
        )
        .unwrap()
    });
    assert_eq!(admin_only_ttl.created_by, admin);
}

#[test]
fn pending_proposals_are_capped_and_slots_free_on_cancel_or_execute() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);

    for id in 0..64_u64 {
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                id,
                GovernanceAction::RemoveProxy(Asset::Other(Symbol::new(&env, "BTC"))),
                0,
            )
            .unwrap();
        });
    }
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::active_ids(env.clone()).len()
        }),
        64
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::next_proposal_id(env.clone()).unwrap()
        }),
        64
    );

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin.clone(),
                64,
                GovernanceAction::RemoveProxy(Asset::Other(Symbol::new(&env, "BTC"))),
                0,
            )
        }),
        Err(GovernanceError::InvalidInput)
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::next_proposal_id(env.clone()).unwrap()
        }),
        64
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::active_ids(env.clone()).len()
        }),
        64
    );

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::cancel_proposal(env.clone(), admin.clone(), 0).unwrap();
    });
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin.clone(),
            64,
            GovernanceAction::RemoveProxy(Asset::Other(Symbol::new(&env, "BTC"))),
            0,
        )
        .unwrap();
    });
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::active_ids(env.clone()).len()
        }),
        64
    );

    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::execute_proposal(env.clone(), admin.clone(), 1).unwrap();
    });
    env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin,
            65,
            GovernanceAction::RemoveProxy(Asset::Other(Symbol::new(&env, "BTC"))),
            0,
        )
        .unwrap();
    });
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::active_ids(env.clone()).len()
        }),
        64
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::next_proposal_id(env.clone()).unwrap()
        }),
        66
    );
}

#[test]
fn upgrade_is_admin_only_and_has_independent_ttl() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(10);
    let non_admin = Address::generate(&env);
    let wasm_hash = BytesN::from_array(&env, &[7_u8; 32]);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                non_admin,
                0,
                GovernanceAction::Upgrade(wasm_hash.clone()),
                0,
            )
        }),
        Err(GovernanceError::Unauthorized)
    );

    let set_upgrade_ttl = submit_now(
        &env,
        &governance_id,
        &admin,
        GovernanceAction::SetActionTtl(OperationKind::Upgrade, 77),
    );
    env.ledger().set(LedgerInfo {
        timestamp: 101,
        protocol_version: 25,
        sequence_number: 101,
        ..Default::default()
    });
    accept_now(&env, &governance_id, &admin, set_upgrade_ttl);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::Upgrade).unwrap()
        }),
        77
    );
    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::get_operation_ttl(env.clone(), OperationKind::SetProxy).unwrap()
        }),
        10
    );

    let proposal = env.as_contract(&governance_id, || {
        ProxyOracleGovernance::create_proposal(
            env.clone(),
            admin,
            1,
            GovernanceAction::Upgrade(wasm_hash),
            0,
        )
        .unwrap()
    });
    assert_eq!(proposal.ttl_ns, 77);
}

#[test]
fn upgrade_zero_hash_is_rejected_before_silent_acceptance() {
    let (env, admin, _proxy_id, governance_id, _proxy) = setup_with_ttl(0);
    let zero_hash = BytesN::from_array(&env, &[0_u8; 32]);

    assert_eq!(
        env.as_contract(&governance_id, || {
            ProxyOracleGovernance::create_proposal(
                env.clone(),
                admin,
                0,
                GovernanceAction::Upgrade(zero_hash),
                0,
            )
        }),
        Err(GovernanceError::InvalidInput)
    );
}
