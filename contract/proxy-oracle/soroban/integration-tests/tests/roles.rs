#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Groups H + I — Role-based authorization + last-admin protection.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol, Vec as SVec};
use templar_primitives::Decimal;
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, ProxyConfig, RearmConfig, SetEnforcedConfig, SorobanDecimal,
    SourceConfig, StepwiseChangeConfig,
};
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, Role};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

fn sample_setproxy(b: &Bootstrap, label: &str) -> GovernanceAction {
    let asset = templar_proxy_oracle_soroban_common::Asset::Other(Symbol::new(&b.env, label));
    let mut sources = SVec::new(&b.env);
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
fn role_holders_can_execute_their_role_specific_actions() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let manager = Address::generate(&b.env);
    let tripper = Address::generate(&b.env);
    let op = Address::generate(&b.env);
    b.grant_role(&manager, Role::ProxyConfigurationManager);
    b.grant_role(&tripper, Role::ManualTripper);
    b.grant_role(&op, Role::CircuitBreakerOperator);

    // PCM: ConfigureBreakers, AddBreaker, SetProxy
    b.submit_and_execute(
        &manager,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    b.submit_and_execute(
        &manager,
        GovernanceAction::AddBreaker(
            b.asset_btc.clone(),
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF),
            }),
        ),
    );
    b.submit_and_execute(&manager, sample_setproxy(&b, "ETH"));

    // Operator: Rearm, SetEnforced
    b.submit_and_execute(
        &op,
        GovernanceAction::SetEnforced(
            b.asset_btc.clone(),
            0,
            SetEnforcedConfig { is_enforced: false },
        ),
    );
    b.submit_and_execute(
        &op,
        GovernanceAction::Rearm(
            b.asset_btc.clone(),
            0,
            RearmConfig {
                armed_after_secs: 0,
                accepted_history_source_code: 0,
            },
        ),
    );

    // Tripper: SetManualTrip
    b.submit_and_execute(
        &tripper,
        GovernanceAction::SetManualTrip(tripper.clone(), b.asset_btc.clone(), true, None),
    );
}

#[test]
fn cross_role_actions_are_denied() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let manager = Address::generate(&b.env);
    let tripper = Address::generate(&b.env);
    b.grant_role(&manager, Role::ProxyConfigurationManager);
    b.grant_role(&tripper, Role::ManualTripper);

    // Tripper attempting PCM action.
    assert!(b
        .governance
        .try_submit(&tripper, &sample_setproxy(&b, "ETH"))
        .is_err());

    // Manager attempting tripper action.
    let manual_trip =
        GovernanceAction::SetManualTrip(manager.clone(), b.asset_btc.clone(), true, None);
    assert!(b.governance.try_submit(&manager, &manual_trip).is_err());
}

#[test]
fn admin_can_execute_any_action() {
    // The bootstrap admin holds only Role::Admin, but Admin should be able to
    // execute every action variant. This is the override branch.
    let b = Bootstrap::new();
    b.configure_default_feed();
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::ConfigureBreakers(b.asset_btc.clone(), 0, 8),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::AddBreaker(
            b.asset_btc.clone(),
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(&b.env, Decimal::ONE_HALF),
            }),
        ),
    );
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetManualTrip(b.admin.clone(), b.asset_btc.clone(), true, None),
    );
}

#[test]
fn multi_role_membership_grants_both_powers() {
    let b = Bootstrap::new();
    b.configure_default_feed();
    let dual = Address::generate(&b.env);
    b.grant_role(&dual, Role::ProxyConfigurationManager);
    b.grant_role(&dual, Role::ManualTripper);

    // Both kinds of actions succeed for the same key.
    b.submit_and_execute(&dual, sample_setproxy(&b, "ETH"));
    b.submit_and_execute(
        &dual,
        GovernanceAction::SetManualTrip(dual.clone(), b.asset_btc.clone(), true, None),
    );

    let roles = b.governance.get_roles(&dual);
    assert_eq!(roles.len(), 2);
    assert!(b
        .governance
        .has_role(&dual, &Role::ProxyConfigurationManager));
    assert!(b.governance.has_role(&dual, &Role::ManualTripper));
}

#[test]
fn revoking_one_role_does_not_touch_the_others() {
    let b = Bootstrap::new();
    let dual = Address::generate(&b.env);
    b.grant_role(&dual, Role::ProxyConfigurationManager);
    b.grant_role(&dual, Role::ManualTripper);

    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetRole(dual.clone(), Role::ManualTripper, false),
    );

    assert!(!b.governance.has_role(&dual, &Role::ManualTripper));
    assert!(b
        .governance
        .has_role(&dual, &Role::ProxyConfigurationManager));
}

#[test]
fn revoking_the_last_admin_is_rejected() {
    // The last-admin guard fires at execute time (inside roles::revoke), not
    // at proposal-create time — so submit succeeds but accept must reject.
    let b = Bootstrap::new();
    let id = b.governance.submit(
        &b.admin,
        &GovernanceAction::SetRole(b.admin.clone(), Role::Admin, false),
    );
    let result = b.governance.try_accept(&b.admin, &id);
    assert!(result.is_err(), "last-admin revoke must fail at execute");
    assert!(b.governance.has_role(&b.admin, &Role::Admin));
}

#[test]
fn revoking_a_non_final_admin_succeeds() {
    let b = Bootstrap::new();
    let admin2 = Address::generate(&b.env);
    b.grant_role(&admin2, Role::Admin);

    b.submit_and_execute(
        &admin2,
        GovernanceAction::SetRole(b.admin.clone(), Role::Admin, false),
    );

    assert!(!b.governance.has_role(&b.admin, &Role::Admin));
    assert!(b.governance.has_role(&admin2, &Role::Admin));
    assert_eq!(b.governance.list_role(&Role::Admin).len(), 1);
}
