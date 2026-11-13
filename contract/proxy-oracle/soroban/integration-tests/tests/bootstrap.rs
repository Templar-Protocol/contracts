#![allow(
    clippy::should_panic_without_expect,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::large_digit_groups,
    clippy::explicit_iter_loop
)]

//! Group A — Deploy + bootstrap chain.
//!
//! Verifies the harness constructs a self-consistent three-contract +
//! upstream system: runtime owner = governance, governance admin = `admin`,
//! adapter owner = `admin`, runtime knows its base asset.

use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, IntoVal,
};
use templar_proxy_oracle_soroban_governance_common::{GovernanceAction, OperationKind, Role};
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;

#[test]
fn bootstrap_deploys_three_contracts_and_links_them() {
    let b = Bootstrap::new();

    // Ownership wiring.
    assert_eq!(b.runtime.get_owner(), Some(b.governance_id.clone()));
    assert_eq!(b.adapter.get_owner(), Some(b.admin.clone()));

    // Governance knows its target.
    assert_eq!(b.governance.proxy_oracle(), b.runtime_id);

    // Admin role is granted to the bootstrap admin.
    assert!(b.governance.has_role(&b.admin, &Role::Admin));

    // Runtime knows its base asset; no feeds configured yet.
    assert_eq!(b.runtime.source_base(), Some(b.base_usd.clone()));
    assert_eq!(b.runtime.registered_assets().len(), 0);

    // Adapter remembers its config.
    let cfg = b.adapter.config().unwrap();
    assert_eq!(cfg.parent_oracle, b.runtime_id);
    assert_eq!(cfg.asset, b.asset_btc);
    assert_eq!(cfg.base, b.base_usd);
}

fn authorize_governance_call(
    b: &Bootstrap,
    caller: &Address,
    function: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
) {
    b.env.mock_auths(&[MockAuth {
        address: caller,
        invoke: &MockAuthInvoke {
            contract: &b.governance_id,
            fn_name: function,
            args,
            sub_invokes: &[],
        },
    }]);
}

#[allow(
    clippy::too_many_lines,
    reason = "Exercise the authenticated handoff and final timelock hardening on the same fresh stack"
)]
#[test]
fn fresh_stack_activates_authenticated_admin_and_hardens_bootstrap_timelocks() {
    let b = Bootstrap::new();
    // Bootstrap performs the ordinary transfer + acceptance, not a storage
    // override. Check both sides and the immutable consumer binding.
    assert_eq!(b.governance.proxy_oracle(), b.runtime_id);
    assert_eq!(b.runtime.get_owner(), Some(b.governance_id.clone()));
    let adapter = b.adapter.config().unwrap();
    assert_eq!(adapter.parent_oracle, b.runtime_id);
    assert_eq!(adapter.asset, b.asset_btc);
    assert_eq!(Some(adapter.base), b.runtime.source_base());

    let new_admin = Address::generate(&b.env);
    let action = GovernanceAction::SetRole(new_admin.clone(), Role::Admin, true);
    let id = b.governance.next_proposal_id();
    authorize_governance_call(
        &b,
        &b.admin,
        "create_proposal",
        (&b.admin, id, &action, 0_u64).into_val(&b.env),
    );
    b.governance.create_proposal(&b.admin, &id, &action, &0);
    let execute = MockAuthInvoke {
        contract: &b.governance_id,
        fn_name: "execute_proposal",
        args: (&b.admin, id).into_val(&b.env),
        sub_invokes: &[],
    };
    b.env.mock_auths(&[MockAuth {
        address: &b.admin,
        invoke: &execute,
    }]);
    assert!(b.governance.try_execute_proposal(&b.admin, &id).is_err());
    assert!(!b.governance.has_role(&new_admin, &Role::Admin));
    assert!(b.governance.get_proposal(&id).is_some());
    b.env.mock_auths(&[
        MockAuth {
            address: &b.admin,
            invoke: &execute,
        },
        MockAuth {
            address: &new_admin,
            invoke: &execute,
        },
    ]);
    b.governance.execute_proposal(&b.admin, &id);
    assert!(b.governance.has_role(&new_admin, &Role::Admin));

    // Demonstrate the new Admin's own usable authorization while hardening
    // every operation. Harden SetActionTtl last so the deliberate zero-delay
    // bootstrap can finish without pretending queued TTLs are retroactive.
    let production_ttl_ns = 86_400_000_000_000;
    let operations = [
        OperationKind::SetProxy,
        OperationKind::RemoveProxy,
        OperationKind::ConfigureBreakers,
        OperationKind::AddBreaker,
        OperationKind::RemoveBreaker,
        OperationKind::Rearm,
        OperationKind::SetEnforced,
        OperationKind::SetManualTrip,
        OperationKind::TransferOwnership,
        OperationKind::AcceptOwnership,
        OperationKind::RenounceOwnership,
        OperationKind::SetRole,
        OperationKind::Upgrade,
        OperationKind::SetActionTtl,
    ];
    for kind in operations {
        assert_eq!(b.governance.get_operation_ttl(&kind), 0);
        let action = GovernanceAction::SetActionTtl(kind, production_ttl_ns);
        let id = b.governance.next_proposal_id();
        authorize_governance_call(
            &b,
            &new_admin,
            "create_proposal",
            (&new_admin, id, &action, 0_u64).into_val(&b.env),
        );
        let proposal = b.governance.create_proposal(&new_admin, &id, &action, &0);
        assert_eq!(proposal.ttl_ns, 0);
        authorize_governance_call(
            &b,
            &new_admin,
            "execute_proposal",
            (&new_admin, id).into_val(&b.env),
        );
        b.governance.execute_proposal(&new_admin, &id);
        assert_eq!(b.governance.get_operation_ttl(&kind), production_ttl_ns);
    }
    for kind in operations {
        assert_eq!(b.governance.get_operation_ttl(&kind), production_ttl_ns);
    }
    let id = b.governance.next_proposal_id();
    let action = GovernanceAction::SetManualTrip(b.asset_btc.clone(), true, None);
    authorize_governance_call(
        &b,
        &new_admin,
        "create_proposal",
        (&new_admin, id, &action, 0_u64).into_val(&b.env),
    );
    assert_eq!(
        b.governance
            .create_proposal(&new_admin, &id, &action, &0)
            .ttl_ns,
        production_ttl_ns
    );
    authorize_governance_call(
        &b,
        &new_admin,
        "execute_proposal",
        (&new_admin, id).into_val(&b.env),
    );
    assert!(b.governance.try_execute_proposal(&new_admin, &id).is_err());
}
