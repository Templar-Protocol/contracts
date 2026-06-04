use near_sdk::{
    json_types::{Base64VecU8, U128},
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, AccountId, NearToken,
};
use near_sdk_contract_tools::rbac::Rbac;
use templar_common::Nanoseconds;
use templar_proxy_oracle_near_governance_common::{Operation, OperationKind, Role, TtlConfig};

use crate::{Contract, ProxyGovernanceInterface};

fn default_ttls() -> TtlConfig {
    TtlConfig {
        set_proxy: Nanoseconds::from_secs(24 * 60 * 60),
        configure_circuit_breakers: Nanoseconds::from_secs(24 * 60 * 60),
        add_circuit_breaker: Nanoseconds::from_secs(24 * 60 * 60),
        remove_circuit_breaker: Nanoseconds::from_secs(24 * 60 * 60),
        set_manual_trip: Nanoseconds::zero(),
        rearm: Nanoseconds::zero(),
        set_enforced: Nanoseconds::zero(),
        set_action_ttl: Nanoseconds::from_secs(48 * 60 * 60),
        set_role: Nanoseconds::from_secs(24 * 60 * 60),
        admin_upgrade: Nanoseconds::from_secs(24 * 60 * 60),
        admin_function_call: Nanoseconds::from_secs(24 * 60 * 60),
    }
}

fn contract() -> Contract {
    Contract::new(
        "proxy.near".parse().unwrap(),
        "admin.near".parse().unwrap(),
        default_ttls(),
    )
}

fn grant_role(contract: &mut Contract, account_id: &str, role: Role) {
    <Contract as Rbac>::add_role(contract, &account_id.parse().unwrap(), &role);
}

fn revoke_role(contract: &mut Contract, account_id: &str, role: Role) {
    <Contract as Rbac>::remove_role(contract, &account_id.parse().unwrap(), &role);
}

fn context_with_admin() -> near_sdk::VMContext {
    VMContextBuilder::new()
        .predecessor_account_id("admin.near".parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build()
}

fn context_with_account(account_id: &str) -> near_sdk::VMContext {
    VMContextBuilder::new()
        .predecessor_account_id(account_id.parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(1))
        .build()
}

#[test]
fn create_and_execute_proposal_with_zero_ttl() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };

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

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(24 * 60 * 60));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute_proposal(0);
    }));
    assert!(result.is_err());
}

#[test]
fn create_proposal_with_custom_ttl() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    let requested = Nanoseconds::from_secs(48 * 60 * 60);
    let proposal = contract.create_proposal(0, operation.clone(), requested);
    assert_eq!(proposal.ttl, requested);
}

#[test]
fn create_proposal_below_minimum_gets_clamped() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    let requested = Nanoseconds::from_secs(60 * 60);
    let proposal = contract.create_proposal(0, operation.clone(), requested);
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(24 * 60 * 60));
}

#[test]
fn get_operation_ttl_returns_configured_ttl() {
    testing_env!(context_with_admin());

    let contract = contract();

    assert_eq!(
        contract.get_operation_ttl(OperationKind::SetRole),
        Nanoseconds::from_secs(24 * 60 * 60)
    );
    assert_eq!(
        contract.get_operation_ttl(OperationKind::Rearm),
        Nanoseconds::zero()
    );
}

#[test]
fn cancel_proposal() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(contract.proposal_count(), 1);

    contract.cancel_proposal(0);
    assert_eq!(contract.proposal_count(), 0);
    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn execute_out_of_order() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let op0 = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };
    let op1 = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };

    contract.create_proposal(0, op0, Nanoseconds::zero());
    contract.create_proposal(1, op1, Nanoseconds::zero());

    contract.execute_proposal(1);
    assert_eq!(contract.get_proposal(1), None);

    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn unauthorized_caller_cannot_create_proposal() {
    testing_env!(context_with_account("unauthorized.near"));

    let mut contract = contract();

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
    assert!(result.is_err());
}

#[test]
fn role_based_caller_can_create_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));

    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };

    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
}

#[test]
fn role_based_caller_can_execute_matching_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };
    contract.create_proposal(0, operation, Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    contract.execute_proposal(0);

    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn role_mismatch_cannot_execute_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };
    contract.create_proposal(0, operation, Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute_proposal(0);
    }));
    assert!(result.is_err());
    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn role_based_caller_can_cancel_matching_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_admin());
    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };
    contract.create_proposal(0, operation, Nanoseconds::zero());

    testing_env!(context_with_account("tripper.near"));
    contract.cancel_proposal(0);

    assert_eq!(contract.get_proposal(0), None);
}

#[test]
fn admin_can_execute_and_cancel_any_role_proposal() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));
    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };
    contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    contract.create_proposal(1, operation, Nanoseconds::zero());

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

    let operation = Operation::SetProxy {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        proxy: None,
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
    assert!(result.is_err());
}

#[test]
fn set_role_grants_adds_and_targeted_revoke_preserves_other_roles() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract.header.ttls.set(
        templar_proxy_oracle_near_governance_common::OperationKind::SetRole,
        Nanoseconds::zero(),
    );
    let account_id: AccountId = "operator.near".parse().unwrap();

    contract.create_proposal(
        0,
        Operation::SetRole {
            account_id: account_id.clone(),
            role: Role::ManualTripper,
            set: true,
        },
        Nanoseconds::zero(),
    );
    contract.execute_proposal(0);
    assert!(contract.has_role(account_id.clone(), Role::ManualTripper));
    assert_eq!(
        contract.list_role(Role::ManualTripper, None, None),
        vec![account_id.clone()]
    );

    contract.create_proposal(
        1,
        Operation::SetRole {
            account_id: account_id.clone(),
            role: Role::CircuitBreakerOperator,
            set: true,
        },
        Nanoseconds::zero(),
    );
    contract.execute_proposal(1);
    assert!(contract.has_role(account_id.clone(), Role::ManualTripper));
    assert!(contract.has_role(account_id.clone(), Role::CircuitBreakerOperator));
    assert_eq!(
        contract.get_roles(account_id.clone()),
        vec![Role::ManualTripper, Role::CircuitBreakerOperator]
    );

    contract.create_proposal(
        2,
        Operation::SetRole {
            account_id: account_id.clone(),
            role: Role::ManualTripper,
            set: false,
        },
        Nanoseconds::zero(),
    );
    contract.execute_proposal(2);
    assert!(!contract.has_role(account_id.clone(), Role::ManualTripper));
    assert!(contract.has_role(account_id, Role::CircuitBreakerOperator));
}

#[test]
fn set_action_ttl_does_not_control_set_role_ttl() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract.header.ttls.set(
        templar_proxy_oracle_near_governance_common::OperationKind::SetActionTtl,
        Nanoseconds::zero(),
    );

    let operation = Operation::SetRole {
        account_id: "operator.near".parse().unwrap(),
        role: Role::ManualTripper,
        set: true,
    };
    let proposal = contract.create_proposal(0, operation, Nanoseconds::zero());
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(24 * 60 * 60));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute_proposal(0);
    }));
    assert!(result.is_err());
}

#[test]
fn set_role_cannot_remove_last_admin() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract.header.ttls.set(
        templar_proxy_oracle_near_governance_common::OperationKind::SetRole,
        Nanoseconds::zero(),
    );

    contract.create_proposal(
        0,
        Operation::SetRole {
            account_id: "admin.near".parse().unwrap(),
            role: Role::Admin,
            set: false,
        },
        Nanoseconds::zero(),
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute_proposal(0);
    }));
    assert!(result.is_err());
    assert!(contract.get_proposal(0).is_some());
    assert!(contract.has_role("admin.near".parse().unwrap(), Role::Admin));
}

#[test]
fn revoked_creator_cannot_execute_later() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));
    let operation = Operation::SetManualTrip {
        id: templar_common::oracle::pyth::PriceIdentifier([0; 32]),
        is_manually_tripped: true,
        metadata: None,
    };
    contract.create_proposal(0, operation, Nanoseconds::zero());

    revoke_role(&mut contract, "tripper.near", Role::ManualTripper);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.execute_proposal(0);
    }));
    assert!(result.is_err());
    assert!(contract.get_proposal(0).is_some());
}

#[test]
fn admin_can_create_admin_function_call_proposal() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::AdminFunctionCall {
        method_name: "own_accept_owner".to_string(),
        args: Base64VecU8(b"{}".to_vec()),
        attached_deposit: U128(0),
        gas: near_sdk::Gas::from_gas(20_000_000_000_000),
    };

    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(24 * 60 * 60));
}

#[test]
fn admin_function_call_execution_dispatches_proxy_call() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract
        .header
        .ttls
        .set(OperationKind::AdminFunctionCall, Nanoseconds::zero());

    let operation = Operation::AdminFunctionCall {
        method_name: "own_accept_owner".to_string(),
        args: Base64VecU8(b"{}".to_vec()),
        attached_deposit: U128(1),
        gas: near_sdk::Gas::from_gas(20_000_000_000_000),
    };

    contract.create_proposal(0, operation, Nanoseconds::zero());
    contract.execute_proposal(0);

    assert_eq!(contract.get_proposal(0), None);

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.receiver_id.as_str(), "proxy.near");
    assert!(receipt.receipt_indices.is_empty());
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
            assert_eq!(*prepaid_gas, near_sdk::Gas::from_gas(20_000_000_000_000));
        }
        action => panic!("expected admin function call, got {action:?}"),
    }
}

#[test]
fn non_admin_cannot_create_admin_function_call_proposal() {
    let mut contract = contract();
    grant_role(
        &mut contract,
        "operator.near",
        Role::ProxyConfigurationManager,
    );

    testing_env!(context_with_account("operator.near"));

    let operation = Operation::AdminFunctionCall {
        method_name: "own_accept_owner".to_string(),
        args: Base64VecU8(b"{}".to_vec()),
        attached_deposit: U128(0),
        gas: near_sdk::Gas::from_gas(20_000_000_000_000),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
    assert!(result.is_err());
}

#[test]
fn admin_upgrade_requires_admin_role_to_create() {
    let mut contract = contract();
    grant_role(&mut contract, "tripper.near", Role::ManualTripper);

    testing_env!(context_with_account("tripper.near"));

    let operation = Operation::AdminUpgrade {
        code: Base64VecU8(vec![0xde, 0xad]),
        migrate_args: Base64VecU8(vec![0xbe, 0xef]),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
    assert!(result.is_err());
}

#[test]
fn admin_can_create_admin_upgrade_proposal() {
    testing_env!(context_with_admin());

    let mut contract = contract();

    let operation = Operation::AdminUpgrade {
        code: Base64VecU8(vec![0xde, 0xad]),
        migrate_args: Base64VecU8(vec![0xbe, 0xef]),
    };

    let proposal = contract.create_proposal(0, operation.clone(), Nanoseconds::zero());
    assert_eq!(proposal.operation, operation);
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(24 * 60 * 60));
}

#[test]
fn admin_upgrade_execution_dispatches_proxy_admin_call() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract
        .header
        .ttls
        .set(OperationKind::AdminUpgrade, Nanoseconds::zero());

    let operation = Operation::AdminUpgrade {
        code: Base64VecU8(vec![0xde, 0xad]),
        migrate_args: Base64VecU8(br#"{"from_version":"v0"}"#.to_vec()),
    };

    contract.create_proposal(0, operation, Nanoseconds::zero());
    contract.execute_proposal(0);

    assert_eq!(contract.get_proposal(0), None);

    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt.receiver_id.as_str(), "proxy.near");
    assert!(receipt.receipt_indices.is_empty());
    assert_eq!(receipt.actions.len(), 1);

    match &receipt.actions[0] {
        MockAction::FunctionCallWeight {
            method_name,
            attached_deposit,
            prepaid_gas,
            ..
        } => {
            assert_eq!(method_name, b"admin_upgrade");
            assert_eq!(*attached_deposit, NearToken::from_yoctonear(0));
            assert_eq!(*prepaid_gas, Contract::GAS_FOR_ADMIN_UPGRADE);
        }
        action => panic!("expected admin_upgrade function call, got {action:?}"),
    }
}

#[test]
fn admin_upgrade_respects_configured_ttl() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract
        .header
        .ttls
        .set(OperationKind::AdminUpgrade, Nanoseconds::from_secs(3600));

    let operation = Operation::AdminUpgrade {
        code: Base64VecU8(vec![0xde, 0xad]),
        migrate_args: Base64VecU8(vec![0xbe, 0xef]),
    };

    let proposal = contract.create_proposal(0, operation, Nanoseconds::zero());
    assert_eq!(proposal.ttl, Nanoseconds::from_secs(3600));
}

#[test]
fn admin_upgrade_rejects_empty_code_in_create() {
    testing_env!(context_with_admin());

    let mut contract = contract();
    contract
        .header
        .ttls
        .set(OperationKind::AdminUpgrade, Nanoseconds::zero());

    let operation = Operation::AdminUpgrade {
        code: Base64VecU8(vec![]),
        migrate_args: Base64VecU8(vec![0x00]),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        contract.create_proposal(0, operation, Nanoseconds::zero());
    }));
    assert!(result.is_err());
}
