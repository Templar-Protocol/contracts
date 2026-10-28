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
    CreateProposalArgs, GovernancePolicy, GovernancePolicyWire, LegacyOperation, MethodPolicy,
    Operation, ReflexiveKind, ReflexiveOperation, ReflexiveTtls, Role, GAS_FOR_ADMIN_UPGRADE,
    MAX_PROPOSAL_TTL,
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
    GovernancePolicyWire {
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
    .try_into()
    .expect("default test policy is within bounds")
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

/// A contract whose policy edits mature immediately, so a test can execute one in the same block.
fn contract_with_immediate_policy_edits() -> Contract {
    let mut contract = contract();
    contract
        .header
        .ttls_mut()
        .set_reflexive_ttl(ReflexiveKind::SetPolicy, Nanoseconds::zero())
        .unwrap();
    contract
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

fn allow_immediate_role_changes(contract: &mut Contract) {
    contract
        .header
        .ttls_mut()
        .set_reflexive_ttl(ReflexiveKind::SetRole, Nanoseconds::zero())
        .unwrap();
}

fn cancelled_ids() -> Vec<u32> {
    get_logs()
        .iter()
        .filter_map(|log| {
            let event: near_sdk::serde_json::Value =
                near_sdk::serde_json::from_str(log.strip_prefix("EVENT_JSON:")?).unwrap();
            (event["event"] == "cancelled")
                .then(|| u32::try_from(event["data"]["id"].as_u64().unwrap()).unwrap())
        })
        .collect()
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
    assert_eq!(policy.reflexive_ttls().set_role, DAY);
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
fn same_role_peer_cannot_cancel_but_can_execute() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    grant_role(&mut contract, "creator.near", Role::ManualTripper);
    grant_role(&mut contract, "peer.near", Role::ManualTripper);

    testing_env!(context_with_account("creator.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());

    testing_env!(context_with_account("peer.near"));
    assert!(panics(|| contract.cancel_proposal(0)));
    assert!(contract.get_proposal(0).is_some());
    assert_eq!(contract.proposal_count(), 1);
    assert!(get_logs().is_empty());
    contract.execute_proposal(0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn creator_can_cancel_after_losing_action_role() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    grant_role(&mut contract, "creator.near", Role::ManualTripper);
    testing_env!(context_with_account("creator.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    // A live method-policy change can remove authorization without revoking membership.
    contract
        .header
        .ttls_mut()
        .set_method_policy(
            "admin_set_manual_trip".to_owned(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::CircuitBreakerOperator,
            }),
        )
        .unwrap();
    assert!(panics(|| contract.execute_proposal(0)));
    contract.cancel_proposal(0);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(contract.get_proposal(0), None);
    assert_eq!(cancelled_ids(), vec![0]);
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

/// Plain Rust calls, so this covers delegation and not the decoder.
#[test]
fn borsh_and_json_entrypoints_produce_the_same_proposal() {
    let operation = admin_upgrade();

    testing_env!(context_with_admin());
    let mut json = contract();
    json.create_proposal(0, operation.clone(), Nanoseconds::zero());
    let json_proposal = json.get_proposal(0);
    let json_logs = get_logs();

    testing_env!(context_with_admin());
    let mut borsh = contract();
    borsh.create_proposal_borsh(CreateProposalArgs {
        id: 0,
        operation: operation.clone(),
        requested_ttl: Nanoseconds::zero(),
    });

    // Two absent proposals, or two empty log sets, would also compare equal.
    assert_eq!(
        json_proposal.as_ref().map(|proposal| &proposal.operation),
        Some(&operation)
    );
    assert_eq!(json_logs.len(), 1);

    assert_eq!(borsh.get_proposal(0), json_proposal);
    assert_eq!(get_logs(), json_logs);
}

#[test]
fn set_role_grants_adds_and_targeted_revoke_preserves_other_roles() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls_mut()
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
        .ttls_mut()
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
        .ttls_mut()
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
        .ttls_mut()
        .set_target_default(MethodPolicy {
            ttl: long,
            role: Role::Admin,
        })
        .unwrap();
    contract
        .header
        .ttls_mut()
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
        .ttls_mut()
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
        .ttls_mut()
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
        .ttls_mut()
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
        .ttls_mut()
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

/// A method ttl above `default_target` passes create-time validation but must revert at execute,
/// where the ceiling invariant is authoritative. Only the v0→v1 migration queues one — see
/// `state::migration::tests::a_pending_ttl_raise_above_the_seeded_default_migrates_over_the_ceiling`.
#[test]
fn set_method_policy_above_default_reverts_at_execute_until_the_default_is_raised() {
    testing_env!(context_with_admin());
    let above_default = DAY.saturating_add(Nanoseconds::from_secs(1));
    let raise_method = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method: "admin_set_proxy".to_owned(),
        policy: Some(MethodPolicy {
            ttl: above_default,
            role: Role::ProxyConfigurationManager,
        }),
    });

    let mut blocked = contract_with_immediate_policy_edits();
    blocked.create_proposal(0, raise_method.clone(), Nanoseconds::zero());
    assert!(panics(|| blocked.execute_proposal(0)));

    let mut unblocked = contract_with_immediate_policy_edits();
    unblocked.create_proposal(
        0,
        Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
            policy: MethodPolicy {
                ttl: above_default,
                role: Role::Admin,
            },
        }),
        Nanoseconds::zero(),
    );
    unblocked.execute_proposal(0);
    unblocked.create_proposal(1, raise_method, Nanoseconds::zero());
    unblocked.execute_proposal(1);
    assert_eq!(
        unblocked.header.ttls().resolve("admin_set_proxy").ttl,
        above_default
    );
}

#[test]
fn set_method_policy_execution_updates_resolution() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract
        .header
        .ttls_mut()
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
        .ttls_mut()
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

#[test]
fn delegate_queue_reserves_admin_recovery_slot_and_revocation_frees_capacity() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    grant_role(&mut contract, "peer.near", Role::ManualTripper);
    for id in 0..63 {
        testing_env!(context_with_account("delegate.near"));
        contract.create_proposal(id, manual_trip(), Nanoseconds::zero());
    }
    testing_env!(context_with_account("delegate.near"));
    let header = contract.header.clone();
    assert!(panics(|| {
        contract.create_proposal(63, manual_trip(), Nanoseconds::zero());
    }));
    assert_eq!(contract.header, header);
    assert_eq!(contract.get_proposal(63), None);
    assert!(get_logs().is_empty());

    testing_env!(context_with_admin());
    contract.create_proposal(
        63,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    assert_eq!(contract.proposal_count(), 64);
    testing_env!(context_with_admin());
    assert!(panics(|| {
        contract.create_proposal(64, manual_trip(), Nanoseconds::zero());
    }));
    assert_eq!(contract.next_proposal_id(), 64);
    assert_eq!(contract.proposal_count(), 64);
    assert!(get_logs().is_empty());

    // A full queue still permits the reserved Admin recovery proposal to execute and purge.
    contract.execute_proposal(63);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(contract.next_proposal_id(), 64);
    assert_eq!(cancelled_ids(), (0..63).collect::<Vec<_>>());
    for id in 0..64 {
        assert_eq!(contract.get_proposal(id), None);
    }
    testing_env!(context_with_account("peer.near"));
    contract.create_proposal(64, manual_trip(), Nanoseconds::zero());
    assert_eq!(contract.proposal_count(), 1);
}

#[test]
fn revocation_preserves_other_roles_other_creators_and_uses_live_method_policy() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    grant_role(&mut contract, "delegate.near", Role::CircuitBreakerOperator);
    grant_role(&mut contract, "peer.near", Role::ManualTripper);
    testing_env!(context_with_account("delegate.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    contract.create_proposal(
        1,
        target(LegacyOperation::SetEnforced {
            id: pid(),
            breaker_id: 0,
            is_enforced: false,
        }),
        Nanoseconds::zero(),
    );
    testing_env!(context_with_account("peer.near"));
    contract.create_proposal(2, manual_trip(), Nanoseconds::zero());

    // Policy changes after creation must govern the purge, not the original role.
    contract
        .header
        .ttls_mut()
        .set_method_policy(
            "admin_set_enforced".to_owned(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::ManualTripper,
            }),
        )
        .unwrap();
    testing_env!(context_with_admin());
    contract.create_proposal(
        3,
        set_role("delegate.near", Role::CircuitBreakerOperator, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(3);
    assert_eq!(contract.list_proposals(None, None), vec![0, 1, 2]);
    assert!(cancelled_ids().is_empty());

    contract.create_proposal(
        4,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(4);
    assert_eq!(contract.list_proposals(None, None), vec![2]);
    assert_eq!(contract.get_proposal(0), None);
    assert_eq!(contract.get_proposal(1), None);
    assert!(contract.get_proposal(2).is_some());
    assert_eq!(cancelled_ids(), vec![0, 1]);
}

#[test]
fn revocation_honors_remaining_admin_override() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::Admin);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    testing_env!(context_with_account("delegate.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    contract.create_proposal(1, set_proxy(), Nanoseconds::zero());
    testing_env!(context_with_admin());
    contract.create_proposal(
        2,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(2);
    assert_eq!(contract.list_proposals(None, None), vec![0, 1]);
    assert!(cancelled_ids().is_empty());
}

#[test]
fn admin_revocation_retains_only_proposals_authorized_by_remaining_delegate_role() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::Admin);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    testing_env!(context_with_account("delegate.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    contract.create_proposal(1, set_proxy(), Nanoseconds::zero());
    contract.create_proposal(
        2,
        set_role("unrelated.near", Role::ManualTripper, true),
        Nanoseconds::zero(),
    );
    testing_env!(context_with_admin());
    contract.create_proposal(
        3,
        set_role("delegate.near", Role::Admin, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(3);
    assert_eq!(contract.list_proposals(None, None), vec![0]);
    assert_eq!(cancelled_ids(), vec![1, 2]);
    assert!(contract.has_role("admin.near".parse().unwrap(), Role::Admin));
    assert!(!contract.has_role("delegate.near".parse().unwrap(), Role::Admin));
}

#[test]
fn no_op_revocation_does_not_sweep_even_now_unauthorized_proposals() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    testing_env!(context_with_account("delegate.near"));
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    revoke_role(&mut contract, "delegate.near", Role::ManualTripper);
    testing_env!(context_with_admin());
    contract.create_proposal(
        1,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(1);
    assert_eq!(contract.list_proposals(None, None), vec![0]);
    assert!(contract.get_proposal(0).is_some());
    assert!(cancelled_ids().is_empty());
}

#[test]
fn replay_after_lost_response_cancel_or_execution_never_creates_a_new_proposal() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    for completed in [false, true] {
        if completed {
            contract.cancel_proposal(0);
        }
        testing_env!(context_with_admin());
        let header = contract.header.clone();
        assert!(panics(|| {
            contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
        }));
        assert_eq!(contract.header, header);
        assert!(get_logs().is_empty());
    }
    contract.create_proposal(1, manual_trip(), Nanoseconds::zero());
    contract.execute_proposal(1);
    testing_env!(context_with_admin());
    let header = contract.header.clone();
    assert!(panics(|| {
        contract.create_proposal_borsh(CreateProposalArgs {
            id: 1,
            operation: manual_trip(),
            requested_ttl: Nanoseconds::zero(),
        });
    }));
    assert_eq!(contract.header, header);
    assert_eq!(contract.proposal_count(), 0);
    assert!(get_logs().is_empty());
}

#[test]
fn contending_expected_id_does_not_create_or_claim_the_winning_proposal() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    grant_role(&mut contract, "alice.near", Role::ManualTripper);
    grant_role(&mut contract, "bob.near", Role::ManualTripper);
    testing_env!(context_with_account("alice.near"));
    let winning = contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    testing_env!(context_with_account("bob.near"));
    let header = contract.header.clone();
    assert!(panics(|| {
        contract.create_proposal(0, manual_trip(), Nanoseconds::from_secs(1));
    }));
    assert_eq!(contract.header, header);
    assert_eq!(contract.get_proposal(0), Some(winning));
    assert_eq!(contract.next_proposal_id(), 1);
    assert_eq!(contract.proposal_count(), 1);
    assert!(get_logs().is_empty());
    assert!(panics(|| contract.cancel_proposal(0)));
}

#[test]
fn pending_byte_limit_reserves_admin_recovery_and_purge_releases_bytes() {
    use templar_proxy_oracle_near_governance_common::{
        proposal_body_size, ADMIN_RESERVED_PROPOSAL_BYTES, MAX_GOVERNANCE_WORKING_SET_BYTES,
    };

    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    let creator: AccountId = "delegate.near".parse().unwrap();
    let mut operation = admin_function_call("admin_set_manual_trip", 0, Gas::from_tgas(1));
    let base = proposal_body_size(&operation, &creator).unwrap();
    let budget = MAX_GOVERNANCE_WORKING_SET_BYTES
        - ADMIN_RESERVED_PROPOSAL_BYTES
        - contract.state_size()
        - 8;
    let Operation::TargetFunctionCall(call) = &mut operation else {
        unreachable!()
    };
    call.args
        .0
        .extend(vec![0; usize::try_from(budget - base).unwrap()]);
    testing_env!(context_with_account("delegate.near"));
    let proposal = contract.create_proposal(0, operation, Nanoseconds::zero());
    assert_eq!(
        near_sdk::borsh::object_length(&proposal).unwrap() as u64,
        budget
    );
    let before = contract.header.clone();
    assert!(panics(|| {
        contract.create_proposal(1, manual_trip(), Nanoseconds::zero());
    }));
    assert_eq!(contract.header, before);
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.body_bytes),
        Some(budget)
    );

    testing_env!(context_with_admin());
    contract.create_proposal(
        1,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(1);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.body_bytes),
        Some(0)
    );
    assert_eq!(cancelled_ids(), vec![0]);
}

#[test]
fn pending_byte_accounting_releases_cancelled_and_executed_bodies() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    let first = contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    let second = contract.create_proposal(1, manual_trip(), Nanoseconds::zero());
    let first_size = near_sdk::borsh::object_length(&first).unwrap() as u64;
    let second_size = near_sdk::borsh::object_length(&second).unwrap() as u64;
    let first_log =
        templar_proxy_oracle_near_governance_common::cancellation_event_size(0, &first.operation)
            .unwrap();
    let second_log =
        templar_proxy_oracle_near_governance_common::cancellation_event_size(1, &second.operation)
            .unwrap();
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.body_bytes),
        Some(first_size + second_size)
    );
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.cancellation_log_bytes),
        Some(first_log + second_log)
    );
    contract.cancel_proposal(0);
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.body_bytes),
        Some(second_size)
    );
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.cancellation_log_bytes),
        Some(second_log)
    );
    contract.execute_proposal(1);
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.body_bytes),
        Some(0)
    );
    assert_eq!(
        Contract::pending_usage().map(|usage| usage.cancellation_log_bytes),
        Some(0)
    );
}

#[test]
fn legacy_queue_must_drain_before_byte_accounting_activates() {
    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(&mut contract, "delegate.near", Role::ManualTripper);
    contract.create_proposal(0, manual_trip(), Nanoseconds::zero());
    contract.create_proposal(
        1,
        set_role("delegate.near", Role::ManualTripper, false),
        Nanoseconds::zero(),
    );
    let key = near_sdk::borsh::to_vec(&crate::StorageKey::PendingProposalBytes).unwrap();
    near_sdk::env::storage_remove(&key);
    let before = contract.header.clone();
    assert!(panics(|| {
        contract.create_proposal(2, manual_trip(), Nanoseconds::zero());
    }));
    assert!(panics(|| contract.execute_proposal(1)));
    assert_eq!(contract.header, before);
    assert!(contract.has_role("delegate.near".parse().unwrap(), Role::ManualTripper));
    contract.cancel_proposal(0);
    contract.execute_proposal(1);
    assert_eq!(contract.proposal_count(), 0);
    contract.create_proposal(2, manual_trip(), Nanoseconds::zero());
    assert!(Contract::pending_usage().is_some());
}

#[test]
fn policy_recovery_budget_covers_longest_key_removal() {
    use templar_proxy_oracle_near_governance_common::{
        proposal_body_size, ADMIN_RESERVED_PROPOSAL_BYTES,
    };

    testing_env!(context_with_admin());
    let mut contract = contract();
    let method = "m".repeat(1_000_000);
    contract
        .header
        .ttls_mut()
        .set_method_policy(
            method.clone(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::Admin,
            }),
        )
        .unwrap();
    contract.assert_policy_recovery();
    let max_admin: AccountId = format!("{}.near", "a".repeat(59)).parse().unwrap();
    let removal = Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
        method,
        policy: None,
    });
    assert!(
        proposal_body_size(&removal, &max_admin).unwrap() + 8
            <= 1_000_000 + ADMIN_RESERVED_PROPOSAL_BYTES
    );
    contract
        .header
        .ttls_mut()
        .set_method_policy("m".repeat(1_000_000), None)
        .unwrap();
    contract
        .header
        .ttls_mut()
        .set_method_policy(
            "m".repeat(1_100_000),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::Admin,
            }),
        )
        .unwrap();
    assert!(panics(|| contract.assert_policy_recovery()));
}

#[test]
fn cancellation_budget_counts_encoded_events_and_reserves_admin_recovery() {
    use near_sdk::AsNep297Event;
    use templar_proxy_oracle_near_governance_common::{
        cancellation_event_size, Event, ADMIN_RESERVED_CANCELLATION_LOG_BYTES,
        MAX_PENDING_CANCELLATION_LOG_BYTES,
    };

    testing_env!(context_with_admin());
    let mut contract = contract();
    allow_immediate_role_changes(&mut contract);
    grant_role(
        &mut contract,
        "delegate.near",
        Role::ProxyConfigurationManager,
    );
    let method = "m\"\\\n".repeat(64);
    contract
        .header
        .ttls_mut()
        .set_method_policy(
            method.clone(),
            Some(MethodPolicy {
                ttl: Nanoseconds::zero(),
                role: Role::ProxyConfigurationManager,
            }),
        )
        .unwrap();
    let operation = admin_function_call(&method, 0, Gas::from_tgas(1));
    let event = Event::Cancelled {
        id: u32::MAX,
        kind: operation.kind(),
        method: operation.method(),
    };
    assert_eq!(
        cancellation_event_size(u32::MAX, &operation).unwrap(),
        event.to_nep297_event().to_event_log().len() as u64
    );
    assert!(
        cancellation_event_size(
            u32::MAX,
            &set_role(
                &format!("{}.near", "a".repeat(59)),
                Role::ManualTripper,
                false
            ),
        )
        .unwrap()
            <= ADMIN_RESERVED_CANCELLATION_LOG_BYTES
    );

    testing_env!(context_with_account("delegate.near"));
    let mut id = 0;
    loop {
        if panics(|| {
            contract.create_proposal(id, operation.clone(), Nanoseconds::zero());
        }) {
            break;
        }
        id += 1;
        assert!(
            id < 63,
            "event budget should bind before the proposal count"
        );
    }
    let usage = Contract::pending_usage().unwrap();
    assert!(
        usage.cancellation_log_bytes
            <= MAX_PENDING_CANCELLATION_LOG_BYTES - ADMIN_RESERVED_CANCELLATION_LOG_BYTES
    );
    assert_eq!(contract.next_proposal_id(), id);
    testing_env!(context_with_admin());
    contract.create_proposal(
        id,
        set_role("delegate.near", Role::ProxyConfigurationManager, false),
        Nanoseconds::zero(),
    );
    contract.execute_proposal(id);
    let usage = Contract::pending_usage().unwrap();
    assert_eq!(usage.body_bytes, 0);
    assert_eq!(usage.cancellation_log_bytes, 0);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(cancelled_ids().len(), usize::try_from(id).unwrap());
}
