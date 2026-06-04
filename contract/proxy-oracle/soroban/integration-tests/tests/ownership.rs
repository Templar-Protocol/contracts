#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group K — Ownership handoff via governance.
//!
//! K1 Two-step transfer through governance proposals.
//! K2 Renounce is permanent: post-renounce owner-only calls panic out-of-band.
//!
//! Note: K2's `live_until_ledger` is currently hardcoded inside
//! `engine.rs::execute_action` to `env.ledger().max_live_until_ledger()`,
//! so the expiration path is not testable without a contract change to expose
//! a configurable window. Skipped intentionally; flagged in the test plan.

use templar_proxy_oracle_soroban_common::Asset;
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_governance_contract::{
    ProxyOracleGovernance, ProxyOracleGovernanceClient,
};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn two_step_ownership_handoff_through_governance() {
    let b = Bootstrap::new();

    // Deploy a second governance contract pointing at the same runtime.
    let governance_v2_id = b
        .env
        .register(ProxyOracleGovernance, (&b.admin, &b.runtime_id, 0_u64));
    let governance_v2 = ProxyOracleGovernanceClient::new(&b.env, &governance_v2_id);

    // v1 initiates the transfer.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::TransferOwnership(governance_v2_id.clone()),
    );

    // v2 accepts.
    let id = governance_v2.next_proposal_id();
    governance_v2.create_proposal(&b.admin, &id, &GovernanceAction::AcceptOwnership(()), &0);
    governance_v2.execute_proposal(&b.admin, &id);

    assert_eq!(b.runtime.get_owner(), Some(governance_v2_id.clone()));

    // v1's mutations now fail because the runtime owner has moved.
    let eth = Asset::Other(soroban_sdk::Symbol::new(&b.env, "ETH"));
    let mut sources = soroban_sdk::Vec::new(&b.env);
    sources.push_back(templar_proxy_oracle_soroban_common::SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: eth.clone(),
    });
    let action = GovernanceAction::SetProxy(
        eth,
        templar_proxy_oracle_soroban_common::ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(300),
            max_clock_drift_secs: Some(60),
        },
    );
    let id_old = b.governance.next_proposal_id();
    b.governance.create_proposal(&b.admin, &id_old, &action, &0);
    let result = b.governance.try_execute_proposal(&b.admin, &id_old);
    assert!(
        result.is_err(),
        "old governance can no longer mutate runtime"
    );
}

#[test]
fn renounce_ownership_is_permanent() {
    let b = Bootstrap::new();

    // Governance renounces ownership of the runtime.
    b.submit_and_execute(&b.admin, GovernanceAction::RenounceOwnership(()));

    assert_eq!(b.runtime.get_owner(), None);
}

#[test]
#[should_panic]
fn renounced_owner_cannot_mutate() {
    // After RenounceOwnership the `#[only_owner]` macro panics with the
    // out-of-band `OwnableError::OwnerNotSet`, not a typed `ContractError`.
    let b = Bootstrap::new();
    b.submit_and_execute(&b.admin, GovernanceAction::RenounceOwnership(()));

    // Any subsequent mutation panics.
    let eth = Asset::Other(soroban_sdk::Symbol::new(&b.env, "ETH"));
    let mut sources = soroban_sdk::Vec::new(&b.env);
    sources.push_back(templar_proxy_oracle_soroban_common::SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: eth.clone(),
    });
    // Try to mutate via governance — collapses to RuntimeFailed at best, or
    // host panic; either way the assertion is the test panics.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            eth,
            templar_proxy_oracle_soroban_common::ProxyConfig {
                sources,
                min_sources: 1,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );
}
