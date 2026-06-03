#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group G — Proposal lifecycle.
//!
//! G1 Submit, fail-before-maturity, advance, execute.
//! G2 Cancel before maturity frees the slot.
//! G3 Pending cap blocks the 65th submission.
//! G4 Out-of-order id is rejected.
//! G5 `get_effective_proposal_ttl` and `get_operation_ttl` reflect SetActionTtl.
//! G6 SetActionTtl's own minimum is the max of (its own TTL, the target's).
//! G7 `requested_ttl` above `MAX_PROPOSAL_TTL_NS` is rejected.

use soroban_sdk::Symbol;
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, SourceConfig};
use templar_proxy_oracle_soroban_governance_common::{
    GovernanceAction, OperationKind, MAX_PROPOSAL_TTL_NS,
};
use templar_proxy_oracle_soroban_integration_tests::common::{ledger, Bootstrap};

fn dummy_setproxy(b: &Bootstrap, label: &str) -> GovernanceAction {
    let asset = Asset::Other(Symbol::new(&b.env, label));
    let mut sources = soroban_sdk::Vec::new(&b.env);
    sources.push_back(SourceConfig {
        oracle: b.upstream_id.clone(),
        asset: asset.clone(),
    });
    GovernanceAction::SetProxy(
        asset,
        ProxyConfig {
            sources,
            min_sources: 1,
            max_age_secs: Some(300),
            max_clock_drift_secs: Some(60),
        },
    )
}

#[test]
fn proposal_matures_then_executes() {
    let b = Bootstrap::new();
    // Lift SetProxy's TTL to 60s so the next SetProxy proposal has to wait.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    let id = b.governance.submit(&b.admin, &dummy_setproxy(&b, "BTC"));

    let early = b.governance.try_accept(&b.admin, &id);
    assert!(early.is_err(), "execute should fail before maturity");

    ledger::advance_secs(&b.env, 65);
    b.governance.accept(&b.admin, &id);

    // active_ids cleared after successful execute.
    assert_eq!(b.governance.active_ids().len(), 0);
}

#[test]
fn cancel_before_maturity_frees_slot() {
    let b = Bootstrap::new();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    let id = b.governance.submit(&b.admin, &dummy_setproxy(&b, "BTC"));
    assert_eq!(b.governance.active_ids().len(), 1);

    b.governance.revoke(&b.admin, &id);

    assert_eq!(b.governance.active_ids().len(), 0);
    assert!(b.governance.get_proposal(&id).is_none());
}

#[test]
fn pending_proposal_cap_blocks_the_65th_submission() {
    let b = Bootstrap::new();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetProxy, 60_000_000_000),
    );

    // 64 pending proposals (MAX_PENDING_PROPOSALS).
    for i in 0..64 {
        let label = std::format!("A{i}");
        b.governance.submit(&b.admin, &dummy_setproxy(&b, &label));
    }
    assert_eq!(b.governance.active_ids().len(), 64);

    // 65th rejected.
    let result = b
        .governance
        .try_submit(&b.admin, &dummy_setproxy(&b, "Z65"));
    assert!(result.is_err());
}

#[test]
fn out_of_order_proposal_id_is_rejected() {
    // The kernel requires `id == next_id` at create time; jumping ahead of
    // `next_proposal_id` is rejected as out-of-order.
    let b = Bootstrap::new();
    let next = b.governance.next_proposal_id();
    let result =
        b.governance
            .try_create_proposal(&b.admin, &(next + 1), &dummy_setproxy(&b, "X"), &0_u64);
    assert!(result.is_err());
    // The canonical next id still works.
    b.governance
        .create_proposal(&b.admin, &next, &dummy_setproxy(&b, "Y"), &0_u64);
}

#[test]
fn effective_and_operation_ttl_views_reflect_set_action_ttl() {
    let b = Bootstrap::new();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::AddBreaker, 30_000_000_000),
    );

    assert_eq!(
        b.governance.get_operation_ttl(&OperationKind::AddBreaker),
        30_000_000_000
    );

    // Even with a smaller requested_ttl, the operation-minimum wins.
    let probe = GovernanceAction::SetActionTtl(OperationKind::AddBreaker, 30_000_000_000);
    let effective = b
        .governance
        .get_effective_proposal_ttl(&probe, &10_000_000_000);
    // SetActionTtl's effective TTL is the max of (its own TTL, the target's).
    assert!(effective >= 30_000_000_000);
}

#[test]
fn set_action_ttl_takes_max_of_own_and_target_ttls() {
    let b = Bootstrap::new();
    // Order matters: change AddBreaker's TTL first (while SetActionTtl's own
    // TTL is still 0 — executes immediately), then change SetActionTtl's own
    // TTL. After that, any subsequent SetActionTtl proposal would itself
    // require 30s.
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::AddBreaker, 10_000_000_000),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetActionTtl(OperationKind::SetActionTtl, 30_000_000_000),
    );

    // A subsequent SetActionTtl(AddBreaker, …) proposal requires max(30s, 10s) = 30s.
    let action = GovernanceAction::SetActionTtl(OperationKind::AddBreaker, 60_000_000_000);
    let effective = b.governance.get_effective_proposal_ttl(&action, &0_u64);
    assert_eq!(effective, 30_000_000_000);
}

#[test]
fn requested_ttl_above_max_is_rejected() {
    let b = Bootstrap::new();
    let result = b.governance.try_create_proposal(
        &b.admin,
        &0_u64,
        &dummy_setproxy(&b, "BTC"),
        &(MAX_PROPOSAL_TTL_NS + 1),
    );
    assert!(result.is_err());
}
