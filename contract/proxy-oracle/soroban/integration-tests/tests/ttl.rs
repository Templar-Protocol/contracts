#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group J — Per-operation TTL adjustment.
//!
//! J1 Tightening SetManualTrip's TTL enables single-tx incident response.
//! J2 A proposal captures its TTL at create time; later TTL changes don't
//!    retroactively make it un-executable.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol, Vec as SVec};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, OperationKind, Role};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn lengthening_manual_trip_ttl_blocks_single_tx_response() {
    use templar_proxy_oracle_soroban_integration_tests::common::ledger;

    let b = Bootstrap::new();
    b.configure_default_feed();
    // Upstream price is needed so refresh doesn't short-circuit to
    // SourceUnavailable before the manual-trip flag is checked.
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let tripper = Address::generate(&b.env);
    b.grant_role(&tripper, Role::ManualTripper);

    // Lengthen SetManualTrip TTL to 60s — incident response now requires a wait.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetManualTrip, 60_000_000_000),
    );

    // Single-tx submit+accept fails because the proposal isn't yet mature.
    let id = b.governance.submit(
        &tripper,
        &GovernanceAction::SetManualTrip(tripper.clone(), b.asset_btc.clone(), true, None),
    );
    assert!(b.governance.try_accept(&tripper, &id).is_err());

    // After the maturity window the trip executes and refresh blocks.
    ledger::advance_secs(&b.env, 65);
    b.governance.accept(&tripper, &id);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));
}

#[test]
fn proposal_captures_ttl_at_create_time() {
    let b = Bootstrap::new();

    // Submit a SetProxy while SetProxy's TTL is still 0 — captures TTL=0.
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
    let id_first = b.governance.submit(&b.admin, &action);

    // Now lengthen SetProxy's TTL for future proposals.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    // The earlier proposal still has TTL=0 captured — executes immediately.
    b.governance.accept(&b.admin, &id_first);
    assert_eq!(b.governance.active_ids().len(), 0);
}
