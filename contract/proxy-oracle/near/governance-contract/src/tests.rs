use std::collections::BTreeMap;

use near_sdk::{
    json_types::{Base64VecU8, U128},
    mock::MockAction,
    test_utils::{get_created_receipts, get_logs, VMContextBuilder},
    testing_env, AccountId, Gas, NearToken,
};
use near_sdk_contract_tools::rbac::Rbac;
use templar_common::{oracle::pyth::PriceIdentifier, upgrade::UpgradeSource, Nanoseconds};
use templar_proxy_oracle_near_governance_common::{
    GovernancePolicy, LegacyOperation, MethodPolicy, Operation, ReflexiveKind, ReflexiveOperation,
    ReflexiveTtls, Role, GAS_FOR_ADMIN_UPGRADE, MAX_PROPOSAL_TTL,
};

use crate::{Contract, ProxyGovernanceInterface};

const DAY: Nanoseconds = Nanoseconds::from_secs(24 * 60 * 60);

fn default_policy() -> GovernancePolicy {
    let mut method_policies = BTreeMap::new();
    for (method, ttl, role) in [
        ("admin_set_proxy", DAY, Role::ProxyConfigurationManager),
        (
            "admin_set_manual_trip",
            Nanoseconds::zero(),
            Role::ManualTripper,
        ),
        (
            "admin_rearm",
            Nanoseconds::zero(),
            Role::CircuitBreakerOperator,
        ),
        (
            "admin_set_enforced",
            Nanoseconds::zero(),
            Role::CircuitBreakerOperator,
        ),
        ("admin_upgrade", DAY, Role::Admin),
    ] {
        method_policies.insert(method.to_owned(), MethodPolicy { ttl, role });
    }
    GovernancePolicy {
        reflexive_ttls: ReflexiveTtls {
            set_policy: Nanoseconds::from_secs(48 * 60 * 60),
            set_role: DAY,
            self_upgrade: DAY,
        },
        default_target: MethodPolicy {
            ttl: DAY,
            role: Role::Admin,
        },
        method_policies,
    }
}

fn contract() -> Contract {
    Contract::new(
        "proxy.near".parse().unwrap(),
        "admin.near".parse().unwrap(),
        default_policy(),
    )
}

fn pid() -> PriceIdentifier {
    PriceIdentifier([0; 32])
}

/// Build a target op from the pre-restructure typed form (exercises the shared mapping).
fn target(legacy: LegacyOperation) -> Operation {
    Operation::try_from(legacy).unwrap()
}

fn manual_trip() -> Operation {
    target(LegacyOperation::SetManualTrip {
        id: pid(),
        is_manually_tripped: true,
        metadata: None,
    })
}

fn set_proxy() -> Operation {
    target(LegacyOperation::SetProxy {
        id: pid(),
        proxy: None,
    })
}

fn admin_upgrade() -> Operation {
    target(LegacyOperation::AdminUpgrade {
        code: UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad])),
        migrate_args: Base64VecU8(vec![0xbe, 0xef]),
    })
}

fn admin_function_call(method: &str, deposit: u128, gas: Gas) -> Operation {
    target(LegacyOperation::AdminFunctionCall {
        method_name: method.to_owned(),
        args: Base64VecU8(b"{}".to_vec()),
        attached_deposit: U128(deposit),
        gas,
    })
}

fn set_role(account_id: &str, role: Role, set: bool) -> Operation {
    Operation::Reflexive(ReflexiveOperation::SetRole {
        account_id: account_id.parse().unwrap(),
        role,
        set,
    })
}

fn grant_role(contract: &mut Contract, account_id: &str, role: Role) {
    <Contract as Rbac>::add_role(contract, &account_id.parse().unwrap(), &role);
}

fn revoke_role(contract: &mut Contract, account_id: &str, role: Role) {
    <Contract as Rbac>::remove_role(contract, &account_id.parse().unwrap(), &role);
}

fn context_with_admin() -> near_sdk::VMContext {
    context_with_account("admin.near")
}

fn context_with_account(account_id: &str) -> near_sdk::VMContext {
    VMContextBuilder::new()
        .predecessor_account_id(account_id.parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build()
}

fn panics(f: impl FnOnce()) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

#[test]
fn create_and_execute_proposal_with_zero_ttl() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let operation = manual_trip();
    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
    assert_eq!(proposal.ttl, Nanoseconds::zero());

    contract.execute_proposal(0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn create_and_execute_proposal_with_nonzero_ttl() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let proposal = contract.create_proposal(0, set_proxy(), Nanoseconds::zero());
    assert_eq!(proposal.ttl, DAY);

    assert!(panics(|| contract.execute_proposal(0)));
}

#[test]
fn create_proposal_with_custom_ttl() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let requested = Nanoseconds::from_secs(48 * 60 * 60);
    let proposal = contract.create_proposal(0, set_proxy(), requested);
    assert_eq!(proposal.ttl, requested);
}

#[test]
fn create_proposal_below_minimum_gets_clamped() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let proposal = contract.create_proposal(0, set_proxy(), Nanoseconds::from_secs(60 * 60));
    assert_eq!(proposal.ttl, DAY);
}

#[test]
fn get_governance_policy_returns_configured_policy() {
    testing_env!(context_with_admin());
    let contract = contract();

    let policy = contract.get_governance_policy();
    assert_eq!(policy.resolve("admin_set_proxy").ttl, DAY);
    assert_eq!(policy.resolve("admin_rearm").ttl, Nanoseconds::zero());
    assert_eq!(policy.reflexive_ttls.set_role, DAY);
    // an unlisted method resolves to the conservative default.
    assert_eq!(policy.resolve("admin_unknown").role, Role::Admin);
}

#[test]
fn cancel_proposal() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    contract.create_proposal(0, set_proxy(), Nanoseconds::zero());
    assert_eq!(contract.proposal_count(), 1);

    contract.cancel_proposal(0);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn execute_out_of_order() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    contract.create_proposal(0, set_proxy(), Nanoseconds::zero());
    contract.create_proposal(1, manual_trip(), Nanoseconds::zero());

    contract.execute_proposal(1);
    assert_eq!(contract.get_proposal(1), None);
    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn unauthorized_caller_cannot_create_proposal() {
    testing_env!(context_with_account("unauthorized.near"));
    let mut contract = contract();
    assert!(panics(|| {
        contract.create_proposal(0, set_proxy(), Nanoseconds::zero());
    }));
}

#[test]
fn role_based_caller_can_create_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);
    testing_env!(context_with_account("tripper.near"));

    let operation = manual_trip();
    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
}

#[test]
fn role_based_caller_can_execute_matching_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    contract.execute_proposal(0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn role_mismatch_cannot_execute_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    contract.create_proposal(0, set_proxy(), Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    assert!(panics(|| contract.execute_proposal(0)));
    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn role_based_caller_can_cancel_matching_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    contract.cancel_proposal(0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn admin_can_execute_and_cancel_any_role_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    contract.create_proposal(1, manual_trip(), Nanoseconds::zero());

    testing_env!(context_with_admin());
    contract.execute_proposal(0);
    contract.cancel_proposal(1);
    assert_eq!(contract.proposal_count(), 0);
}

#[test]
fn role_mismatch_cannot_create_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);
    testing_env!(context_with_account("tripper.near"));

    assert!(panics(|| {
        contract.create_proposal(0, set_proxy(), Nanoseconds::zero());
    }));
}

#[test]
fn set_role_grants_adds_and_targeted_revoke_preserves_other_roles() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetRole, Nanoseconds::zero())
        .unwrap();
    let account_id: AccountId = "operator.near".parse().unwrap();

    contract.create_proposal(
        0,
        set_role("operator.near", Role::ManualTripper, true),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(0);
    assert!(contract.has_role(account_id.clone(), Role::ManualTripper));

    contract.create_proposal(
        1,
        set_role("operator.near", Role::CircuitBreakerOperator, true),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(1);
    assert_eq!(
        contract.get_roles(account_id.clone()),
        vec![Role::ManualTripper, Role::CircuitBreakerOperator]
    );

    contract.create_proposal(
        2,
        set_role("operator.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(2);
    assert!(!contract.has_role(account_id.clone(), Role::ManualTripper));
    assert!(contract.has_role(account_id, Role::CircuitBreakerOperator));
}

#[test]
fn reflexive_timelocks_are_independent() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    // Shortening the policy-edit bucket must not shorten the set-role bucket.
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::zero())
        .unwrap();

    let proposal = contract.create_proposal(
        0,
        set_role("operator.near", Role::ManualTripper, true),
        Nanoseconds::zero(),
    );
    assert_eq!(proposal.ttl, DAY);
    assert!(panics(|| contract.execute_proposal(0)));
}

#[test]
fn shortening_a_reflexive_lock_matures_under_that_lock() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    // self_upgrade longer than the policy-edit lock; shortening it must still wait the full
    // self_upgrade lock, so the ceiling can't be weakened out from under itself.
    let long = Nanoseconds::from_secs(72 * 60 * 60);
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SelfUpgrade, long)
        .unwrap();

    let shorten = Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
        kind: ReflexiveKind::SelfUpgrade,
        ttl: Nanoseconds::zero(),
    });
    let proposal = contract.create_proposal(0, shorten, Nanoseconds::zero());
    assert_eq!(proposal.ttl, long);
    assert!(panics(|| contract.execute_proposal(0)));
}

#[test]
fn shortening_a_method_lock_matures_under_that_lock() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    // A long lock on admin_upgrade (which upgrades the proxy oracle) and a short policy-edit lock.
    let long = Nanoseconds::from_secs(72 * 60 * 60);
    contract
        .header
        .ttls
        .set_target_default(MethodPolicy {
            ttl: long,
            role: Role::Admin,
        })
        .unwrap();
    contract
        .header
        .ttls
        .set_method_policy(
            "admin_upgrade".to_owned(),
            Some(MethodPolicy {
                ttl: long,
                role: Role::Admin,
            }),
        )
        .unwrap();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::from_secs(60 * 60))
        .unwrap();

    // Dropping admin_upgrade's lock to zero must mature under the full admin_upgrade lock, not the
    // short policy-edit lock — the proxy-oracle upgrade path can't be sped up out from under itself.
    let shorten = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_upgrade".to_owned(),
        policy: Some(MethodPolicy {
            ttl: Nanoseconds::zero(),
            role: Role::Admin,
        }),
    });
    let proposal = contract.create_proposal(0, shorten, Nanoseconds::zero());
    assert_eq!(proposal.ttl, long);
    assert!(panics(|| contract.execute_proposal(0)));
}

#[test]
fn set_role_cannot_remove_last_admin() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetRole, Nanoseconds::zero())
        .unwrap();

    contract.create_proposal(
        0,
        set_role("admin.near", Role::Admin, false),
        Nanoseconds::zero(),
    );

    assert!(panics(|| contract.execute_proposal(0)));
    assert!(contract.get_proposal(0).is_some());
    assert!(contract.has_role("admin.near".parse().unwrap(), Role::Admin));
}

#[test]
fn revoked_creator_cannot_execute_later() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());

    revoke_role(&mut contract, "tripper.near", Role::ManualTripper);
    assert!(panics(|| contract.execute_proposal(0)));
    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn admin_can_create_generic_target_call_proposal() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let operation = admin_function_call("own_accept_owner", 0, Gas::from_tgas(20));
    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
    // an unlisted method resolves to the default_target ttl.
    assert_eq!(proposal.ttl, DAY);
}

#[test]
fn generic_target_call_execution_dispatches_proxy_call() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    // List the target method at zero ttl so it matures immediately.
    contract
        .header
        .ttls
        .set_method_policy(
            "own_accept_owner".to_owned(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::Admin,
            }),
        )
        .unwrap();

    contract.create_proposal(
        0,
        admin_function_call("own_accept_owner", 1, Gas::from_tgas(20)),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(0);
    assert_eq!(contract.get_proposal(0), None);

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.receiver_id.as_str(), "proxy.near");
    assert_eq!(receipt.actions.len(), 1);
    match &receipt.actions[0] {
        MockAction::FunctionCallWeight {
            method_name,
            args,
            attached_deposit,
            prepaid_gas,
            ..
        } => {
            assert_eq!(method_name, b"own_accept_owner");
            assert_eq!(args, b"{}");
            assert_eq!(*attached_deposit, NearToken::from_yoctonear(1));
            assert_eq!(*prepaid_gas, Gas::from_tgas(20));
        }
        action => panic!("expected function call, got {action:?}"),
    }
}

#[test]
fn non_admin_cannot_create_unlisted_target_call_proposal() {
    let mut contract = contract();
    grant_role(
        &mut contract,
        "operator.near",
        Role::ProxyConfigurationManager,
    );
    testing_env!(context_with_account("operator.near"));

    assert!(panics(|| {
        contract.create_proposal(
            0,
            admin_function_call("own_accept_owner", 0, Gas::from_tgas(20)),
            Nanoseconds::zero(),
        );
    }));
}

#[test]
fn admin_upgrade_requires_admin_role_to_create() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);
    testing_env!(context_with_account("tripper.near"));

    assert!(panics(|| {
        contract.create_proposal(0, admin_upgrade(), Nanoseconds::zero());
    }));
}

#[test]
fn admin_upgrade_execution_dispatches_target_call_with_upgrade_gas() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls
        .set_method_policy(
            "admin_upgrade".to_owned(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::Admin,
            }),
        )
        .unwrap();

    contract.create_proposal(0, admin_upgrade(), Nanoseconds::zero());
    contract.execute_proposal(0);

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    match &receipts[0].actions[0] {
        MockAction::FunctionCallWeight {
            method_name,
            attached_deposit,
            prepaid_gas,
            ..
        } => {
            assert_eq!(method_name, b"admin_upgrade");
            assert_eq!(*attached_deposit, NearToken::from_yoctonear(0));
            assert_eq!(*prepaid_gas, GAS_FOR_ADMIN_UPGRADE);
        }
        action => panic!("expected admin_upgrade function call, got {action:?}"),
    }
}

#[test]
fn set_method_policy_above_default_reverts_at_execute() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::zero())
        .unwrap();

    // default_target is DAY; a method ttl above it passes create-time validation but must revert at
    // execute (the ceiling invariant is authoritative there).
    let operation = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_set_proxy".to_owned(),
        policy: Some(MethodPolicy {
            ttl: DAY.saturating_add(Nanoseconds::from_secs(1)),
            role: Role::ProxyConfigurationManager,
        }),
    });
    contract.create_proposal(0, operation, Nanoseconds::zero());
    assert!(panics(|| contract.execute_proposal(0)));
}

#[test]
fn set_method_policy_execution_updates_resolution() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::zero())
        .unwrap();

    // Override a previously-unlisted method at the default ttl (so it isn't a shortening, which would
    // gate it) with a non-default role, and confirm resolution picks it up after execution.
    let operation = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_new_method".to_owned(),
        policy: Some(MethodPolicy {
            ttl: DAY,
            role: Role::CircuitBreakerOperator,
        }),
    });
    contract.create_proposal(0, operation, Nanoseconds::zero());
    contract.execute_proposal(0);

    let resolved = contract.get_governance_policy().resolve("admin_new_method");
    assert_eq!(resolved.ttl, DAY);
    assert_eq!(resolved.role, Role::CircuitBreakerOperator);
}

#[test]
fn created_event_carries_kind_and_method() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract.create_proposal(0, set_proxy(), Nanoseconds::zero());

    let logs = get_logs();
    let created = logs
        .iter()
        .find(|log| log.contains("\"event\":\"created\""))
        .expect("created event emitted");
    assert!(created.contains("\"kind\":\"TargetFunctionCall\""));
    assert!(created.contains("\"method\":\"admin_set_proxy\""));
}

#[test]
fn self_upgrade_execution_self_deploys_and_migrates() {
    testing_env!(VMContextBuilder::new()
        .current_account_id("governance.near".parse().unwrap())
        .predecessor_account_id("admin.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build());

    let mut contract = contract();
    contract
        .header
        .ttls
        .set_reflexive_ttl(ReflexiveKind::SelfUpgrade, Nanoseconds::zero())
        .unwrap();

    let operation = Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code: UpgradeSource::Code(Base64VecU8(vec![0xde, 0xad, 0xbe, 0xef])),
        migrate_args: Base64VecU8(br#"{"from_version":"v0"}"#.to_vec()),
    });

    contract.create_proposal(0, operation, Nanoseconds::zero());
    contract.execute_proposal(0);
    assert_eq!(contract.get_proposal(0), None);

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.receiver_id.as_str(), "governance.near");
    assert_eq!(receipt.actions.len(), 2);
    assert!(matches!(
        receipt.actions[0],
        MockAction::DeployContract { .. }
    ));
    match &receipt.actions[1] {
        MockAction::FunctionCallWeight {
            method_name,
            prepaid_gas,
            ..
        } => {
            assert_eq!(method_name, b"migrate");
            assert_eq!(*prepaid_gas, Contract::GAS_FOR_MIGRATE);
        }
        action => panic!("expected migrate function call, got {action:?}"),
    }
}

/// `new` takes an already-parsed policy: near-sdk deserializes its init args into
/// `GovernancePolicy`, which exists only within bounds. An oversized `set_policy` lock would
/// otherwise brick governance — every policy-repair proposal then exceeds `MAX_PROPOSAL_TTL` at
/// create — so such args are rejected before `new` runs at all.
#[test]
fn init_args_carrying_a_bricking_policy_are_rejected_while_parsing() {
    let mut args = near_sdk::serde_json::json!({
        "proxy_oracle_id": "proxy.near",
        "admin_id": "admin.near",
        "policy": default_policy(),
    });
    args["policy"]["reflexive_ttls"]["set_policy"] =
        near_sdk::serde_json::json!(MAX_PROPOSAL_TTL.saturating_add(Nanoseconds::from_secs(1)));

    let error = near_sdk::serde_json::from_value::<GovernancePolicy>(args["policy"].clone())
        .expect_err("an out-of-range policy must not parse");
    assert!(
        error.to_string().contains("exceeds maximum"),
        "unexpected error: {error}"
    );
}

#[test]
fn self_upgrade_rejects_empty_code_in_create() {
    testing_env!(context_with_admin());
    let mut contract = contract();

    let operation = Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
        code: UpgradeSource::Code(Base64VecU8(vec![])),
        migrate_args: Base64VecU8(vec![0x00]),
    });
    assert!(panics(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
}
