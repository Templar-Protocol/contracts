#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Hypernative integration — designated `ManualTripper` key, fast-path TTL,
//! and the negative-permission surface that bounds what such a key can do.
//!
//! These tests model what a Hypernative deployment looks like in our system:
//! a single key (or multisig) is granted `Role::ManualTripper`, and the
//! governance-side `SetManualTrip` TTL is dialed down to zero so emergency
//! trips fire in one logical transaction (submit + execute).
//!
//! Hyper-1 — Single-tx submit+execute trip with TTL=0.
//! Hyper-2 — The same key cannot upgrade, grant roles, configure breakers,
//!           or transfer ownership.
//! Hyper-3 — Untrip via a separate operator preserves event attribution.
//! Hyper-4 — Trip persists across repeated refreshes.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Bytes, BytesN, Symbol};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, RearmConfig, SetEnforcedConfig, SorobanDecimal, StepwiseChangeConfig,
};
use templar_proxy_oracle_soroban_contract::RefreshStatus;
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, OperationKind, Role};
use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap};

fn setup_with_hypernative(b: &Bootstrap) -> Address {
    b.configure_default_feed();
    b.push_upstream_price(&b.asset_btc, 5_000_000_000, 100);
    let _ = b.refresh_one(&b.asset_btc);

    // SetActionTtl for SetManualTrip is already 0 by default in the bootstrap,
    // but make it explicit so this test reads as "Hypernative deployment".
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetManualTrip, 0),
    );

    let hypernative = Address::generate(&b.env);
    b.grant_role(&hypernative, Role::ManualTripper);
    hypernative
}

#[test]
fn hypernative_key_can_trip_in_single_tx() {
    let b = Bootstrap::new();
    let hypernative = setup_with_hypernative(&b);

    let metadata = Bytes::from_slice(&b.env, b"hypernative incident #1234");
    // The two calls happen sequentially within the same logical transaction:
    // submit immediately followed by accept, no maturity wait.
    let id = b.governance.submit(
        &hypernative,
        &GovernanceAction::SetManualTrip(
            hypernative.clone(),
            b.asset_btc.clone(),
            true,
            Some(metadata),
        ),
    );
    b.governance.accept(&hypernative, &id);

    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Blocked(_)
    ));
    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(view.is_manually_tripped);
}

#[test]
fn hypernative_key_cannot_do_anything_else() {
    let b = Bootstrap::new();
    let hypernative = setup_with_hypernative(&b);

    let other_asset = templar_proxy_oracle_soroban_common::Asset::Other(Symbol::new(&b.env, "ETH"));
    let mut sources = soroban_sdk::Vec::new(&b.env);
    sources.push_back(templar_proxy_oracle_soroban_common::SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: other_asset.clone(),
    });
    let setproxy = GovernanceAction::SetProxy(
        other_asset,
        templar_proxy_oracle_soroban_common::ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(300),
            max_clock_drift_secs: Some(60),
        },
    );

    let denied: &[GovernanceAction] = &[
        setproxy,
        GovernanceAction::RemoveProxy(b.asset_btc.clone()),
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
        GovernanceAction::AddBreaker(
            b.asset_btc.clone(),
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF),
            }),
        ),
        GovernanceAction::RemoveBreaker(b.asset_btc.clone(), 0),
        GovernanceAction::Rearm(
            b.asset_btc.clone(),
            0,
            RearmConfig {
                armed_after_secs: 0,
                accepted_history_source_code: 0,
            },
        ),
        GovernanceAction::SetEnforced(
            b.asset_btc.clone(),
            0,
            SetEnforcedConfig { is_enforced: false },
        ),
        GovernanceAction::SetActionTtl(OperationKind::SetManualTrip, 60_000_000_000),
        GovernanceAction::SetRole(hypernative.clone(), Role::Admin, true),
        GovernanceAction::TransferOwnership(Address::generate(&b.env)),
        GovernanceAction::AcceptOwnership(()),
        GovernanceAction::RenounceOwnership(()),
        GovernanceAction::Upgrade(BytesN::<32>::from_array(&b.env, &[1_u8; 32])),
    ];

    for action in denied.iter() {
        let result = b.governance.try_submit(&hypernative, action);
        assert!(
            result.is_err(),
            "Hypernative key should not be able to submit {:?}",
            action.kind()
        );
    }
}

#[test]
fn separate_operator_untrips_after_hypernative_trip() {
    let b = Bootstrap::new();
    let hypernative = setup_with_hypernative(&b);

    // Hypernative trips.
    let trip_id = b.governance.submit(
        &hypernative,
        &GovernanceAction::SetManualTrip(hypernative.clone(), b.asset_btc.clone(), true, None),
    );
    b.governance.accept(&hypernative, &trip_id);

    // A separate manual operator (with the same role) untrips after review.
    let operator = Address::generate(&b.env);
    b.grant_role(&operator, Role::ManualTripper);

    let untrip_id = b.governance.submit(
        &operator,
        &GovernanceAction::SetManualTrip(
            operator.clone(),
            b.asset_btc.clone(),
            false,
            Some(Bytes::from_slice(&b.env, b"manual review approved")),
        ),
    );
    b.governance.accept(&operator, &untrip_id);

    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(!view.is_manually_tripped);
    assert!(matches!(
        b.refresh_one(&b.asset_btc),
        RefreshStatus::Accepted(_)
    ));
}

#[test]
fn trip_persists_across_repeated_refreshes() {
    let b = Bootstrap::new();
    let hypernative = setup_with_hypernative(&b);

    let id = b.governance.submit(
        &hypernative,
        &GovernanceAction::SetManualTrip(hypernative.clone(), b.asset_btc.clone(), true, None),
    );
    b.governance.accept(&hypernative, &id);

    for i in 0..5 {
        ledger::advance_secs(&b.env, 1);
        let now = b.env.ledger().timestamp();
        b.push_upstream_price(&b.asset_btc, 5_000_000_000 + i128::from(i), now);
        assert!(matches!(
            b.refresh_one(&b.asset_btc),
            RefreshStatus::Blocked(_)
        ));
    }

    let view = b.runtime.get_breaker_set_view(&b.asset_btc).unwrap();
    assert!(view.is_manually_tripped);
}
