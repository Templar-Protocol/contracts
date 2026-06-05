#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group Q — TTL extension surface.
//!
//! Q2 `runtime.extend_ttl()` is publicly callable and renews every
//!    asset-keyed persistent entry.
//! Q3 `governance.extend_ttl()` is publicly callable and renews governance
//!    state plus active proposal bodies.
//!
//! Q1 (instance-TTL fail-closed simulation) is intentionally omitted —
//! advancing past `max_entry_ttl` in the testutils harness without leaking
//! host internals isn't reliable; the contract's `MissingConfig` / storage
//! error paths are still covered by the unit tests inside each crate.

use soroban_sdk::{Symbol, Vec as SVec};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn runtime_extend_ttl_is_public_and_succeeds_with_assets_configured() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let eth = Asset::Other(Symbol::new(&b.env, "ETH"));
    let mut sources = SVec::new(&b.env);
    sources.push_back(SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: eth.clone(),
    });
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            eth,
            ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );

    // No auth required.
    b.runtime.extend_ttl();
    // Still callable a second time.
    b.runtime.extend_ttl();
}

#[test]
fn governance_extend_ttl_is_public() {
    let b = Bootstrap::new();

    b.governance.extend_ttl();
    b.governance.extend_ttl();
}
