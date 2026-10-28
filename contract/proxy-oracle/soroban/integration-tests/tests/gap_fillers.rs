#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Gap fillers identified during the coverage-matrix review.
//!
//! R1 — RemoveProxy round-trip
//! R2 — `get_proposal(id)` returns the current action + maturity ledger
//! R3 — `list_role(Admin)` reflects post-grant/post-revoke membership

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, ProxyConfig, RearmConfig, SetEnforcedConfig, SorobanDecimal,
    StepwiseChangeConfig,
};
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, GovernanceError, OperationKind, Role,
};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[allow(
    clippy::too_many_lines,
    reason = "Stale operations must target the same removed and recreated breaker allocator"
)]
#[test]
fn remove_recreate_hides_tombstone_and_rejects_stale_governed_breaker_operations() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let config = b.runtime.get_proxy(&b.asset_btc).unwrap();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    let rule = CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
        max_relative_change: SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF),
    });
    for expected_next_id in [1, 2] {
        b.submit_and_execute(
            &b.admin,
            GovernanceAction::AddBreaker(b.asset_btc.clone(), rule.clone()),
        );
        assert_eq!(
            b.runtime
                .get_breaker_set_view(&b.asset_btc)
                .unwrap()
                .next_id,
            expected_next_id
        );
    }
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);

    // Pre-conditions.
    assert!(b.runtime.get_proxy(&b.asset_btc).is_some());
    assert!(b.runtime.get_cached(&b.asset_btc).is_some());
    assert_eq!(b.runtime.registered_assets().len(), 1);

    b.submit_and_execute(&b.admin, GovernanceAction::RemoveProxy(b.asset_btc.clone()));

    assert!(b.runtime.get_proxy(&b.asset_btc).is_none());
    assert!(b.runtime.get_cached(&b.asset_btc).is_none());
    assert!(b.runtime.get_breaker_set_view(&b.asset_btc).is_none());
    assert_eq!(b.runtime.registered_assets().len(), 0);
    assert!(b.runtime.aggregated_history(&b.asset_btc, &32).is_none());

    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(b.asset_btc.clone(), config),
    );
    assert_eq!(
        b.runtime
            .get_breaker_set_view(&b.asset_btc)
            .unwrap()
            .next_id,
        2
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::AddBreaker(b.asset_btc.clone(), rule),
    );
    assert_eq!(
        b.runtime
            .get_breaker_set_view(&b.asset_btc)
            .unwrap()
            .next_id,
        3
    );
    b.refresh_one(&b.asset_btc);
    let cached = b.runtime.aggregated_latest(&b.asset_btc);
    for action in [
        GovernanceAction::RemoveBreaker(b.asset_btc.clone(), 0),
        GovernanceAction::Rearm(
            b.asset_btc.clone(),
            0,
            RearmConfig {
                arming_delay_secs: 0,
            },
        ),
        GovernanceAction::SetEnforced(
            b.asset_btc.clone(),
            0,
            SetEnforcedConfig { is_enforced: false },
        ),
    ] {
        let id = b.governance.next_proposal_id();
        b.governance.create_proposal(&b.admin, &id, &action, &0);
        assert_eq!(
            b.governance.try_execute_proposal(&b.admin, &id),
            Err(Ok(GovernanceError::RuntimeFailed))
        );
        assert_eq!(b.governance.get_proposal(&id).unwrap().operation, action);
        assert_eq!(
            b.runtime
                .get_breaker_set_view(&b.asset_btc)
                .unwrap()
                .next_id,
            3
        );
        assert_eq!(
            b.runtime
                .get_breaker_set_view(&b.asset_btc)
                .unwrap()
                .breaker_count,
            1
        );
        assert_eq!(b.runtime.aggregated_latest(&b.asset_btc), cached);
        b.governance.cancel_proposal(&b.admin, &id);
    }
}

#[test]
fn get_proposal_returns_action_and_maturity() {
    let b = Bootstrap::new();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    let asset = Asset::Other(Symbol::new(&b.env, "ETH"));
    let sources = b.source_configs(&asset);
    let action = GovernanceAction::SetProxy(
        asset,
        ProxyConfig {
            aggregator: templar_proxy_oracle_soroban_common::AggregatorConfig::MedianLow(
                templar_proxy_oracle_soroban_common::MedianConfig {
                    sources,
                    min_sources: 3,
                },
            ),
            max_age_secs: 300,
            max_clock_drift_secs: 60,
        },
    );
    let id = b.governance.next_proposal_id();
    b.governance.create_proposal(&b.admin, &id, &action, &0);

    let proposal = b.governance.get_proposal(&id).unwrap();
    assert_eq!(proposal.operation, action);
    // SetProxy's per-kind TTL was raised to 60s above, and requested_ttl is 0,
    // so the captured maturity is exactly that minimum.
    assert_eq!(proposal.ttl_ns, 60_000_000_000);
}

#[test]
fn list_role_reflects_grants_and_revokes() {
    let b = Bootstrap::new();
    assert_eq!(b.governance.list_role(&Role::Admin).len(), 1);

    let new_admin = Address::generate(&b.env);
    b.grant_role(&new_admin, Role::Admin);
    assert_eq!(b.governance.list_role(&Role::Admin).len(), 2);

    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetRole(new_admin.clone(), Role::Admin, false),
    );
    assert_eq!(b.governance.list_role(&Role::Admin).len(), 1);
}
