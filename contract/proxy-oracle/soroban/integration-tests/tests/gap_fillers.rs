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
use soroban_sdk::{Address, Symbol, Vec as SVec};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, OperationKind, Role};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn remove_proxy_clears_proxy_cache_history_and_breakers() {
    let b = Bootstrap::new();
    b.configure_default_feed();
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
}

#[test]
fn get_proposal_returns_action_and_maturity() {
    let b = Bootstrap::new();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    let asset = Asset::Other(Symbol::new(&b.env, "ETH"));
    let mut sources = SVec::new(&b.env);
    sources.push_back(SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: asset.clone(),
    });
    let action = GovernanceAction::SetProxy(
        asset,
        ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(300),
            max_clock_drift_secs: Some(60),
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
