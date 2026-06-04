#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group F — Manual trip (incident response path).
//!
//! F1 ManualTripper trip blocks refresh.
//! F2 Untrip restores refresh.
//! F3 Metadata exceeding `MAX_MANUAL_TRIP_METADATA_LEN` is rejected at create.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Bytes};
use templar_proxy_oracle_soroban_common::MAX_MANUAL_TRIP_METADATA_LEN;
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, Role};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

fn setup_with_tripper(b: &Bootstrap) -> Address {
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);

    let tripper = Address::generate(&b.env);
    b.grant_role(&tripper, Role::ManualTripper);
    tripper
}

#[test]
fn manual_trip_via_governance_blocks_refresh() {
    let b = Bootstrap::new();
    let tripper = setup_with_tripper(&b);

    let metadata = Bytes::from_slice(&b.env, b"alert: exploit suspected");
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, Some(metadata)),
    );

    // A manual trip clears the cache immediately on execution, before any refresh.
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());

    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));
    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(view.is_manually_tripped);
    assert!(view.is_blocking);
    assert!(b.adapter.lastprice(&b.asset_btc).is_none());
}

#[test]
fn untrip_restores_refresh() {
    let b = Bootstrap::new();
    let tripper = setup_with_tripper(&b);

    // Trip.
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, None),
    );
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));

    // Untrip.
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(b.asset_btc.clone(), false, None),
    );
    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(!view.is_manually_tripped);

    // Refresh accepts again.
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn metadata_exceeding_cap_is_rejected() {
    let b = Bootstrap::new();
    let tripper = setup_with_tripper(&b);

    let oversized = Bytes::from_slice(&b.env, &[0_u8; MAX_MANUAL_TRIP_METADATA_LEN + 1]);
    let next_id = b.governance.next_proposal_id();
    let result = b.governance.try_create_proposal(
        &tripper,
        &next_id,
        &GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, Some(oversized)),
        &0,
    );
    assert!(result.is_err());
}
