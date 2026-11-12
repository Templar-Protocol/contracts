use near_sdk::{
    borsh, env,
    mock::MockAction,
    test_utils::{get_created_receipts, VMContextBuilder},
    testing_env, AccountId, Gas, NearToken, PromiseOrValue, PromiseResult,
};
use near_sdk_contract_tools::owner::OwnerExternal;
use rstest::rstest;
use templar_common::registry::{Deployment, RegistryEntryView, Role, VersionSource};

use crate::{Contract, RegistryEntry};

const OWNER: &str = "owner.near";
const REGISTRY: &str = "registry.near";
const PAYER: &str = "payer.near";
const HEIGHT: u64 = 42;
const SOLVENCY_ERROR: &str = "Insufficient registry balance to cover storage liabilities";

fn account(name: &str) -> AccountId {
    name.parse().unwrap()
}

fn context(predecessor: &str, deposit: NearToken, balance: NearToken) -> VMContextBuilder {
    let mut builder = VMContextBuilder::new();
    builder
        .current_account_id(account(REGISTRY))
        .signer_account_id(account(predecessor))
        .predecessor_account_id(account(predecessor))
        .attached_deposit(deposit)
        // VMLogic adds the attached deposit; scenarios supply a post-deposit balance.
        .account_balance(balance.checked_sub(deposit).unwrap())
        .storage_usage(env::storage_usage())
        .block_height(HEIGHT)
        .prepaid_gas(Gas::from_tgas(300));
    builder
}

fn receipt(predecessor: &str, deposit: NearToken, balance: NearToken) {
    testing_env!(context(predecessor, deposit, balance).build());
}

fn storage_cost(bytes: u64) -> NearToken {
    env::storage_byte_cost()
        .checked_mul(u128::from(bytes))
        .unwrap()
}

fn physical_storage_cost() -> NearToken {
    storage_cost(env::storage_usage())
}

fn assert_panics(expected: &str, action: impl FnOnce()) {
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_err();
    let actual = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(
        actual.contains(expected),
        "expected {expected:?}, got {actual:?}"
    );
}

fn transfers() -> Vec<(AccountId, NearToken)> {
    get_created_receipts()
        .into_iter()
        .flat_map(|receipt| {
            receipt.actions.into_iter().filter_map(move |action| {
                if let MockAction::Transfer { deposit, .. } = action {
                    Some((receipt.receiver_id.clone(), deposit))
                } else {
                    None
                }
            })
        })
        .collect()
}

/// Measure the real standard Owner endpoints' maximum footprint, rather than
/// asking the solvency helper how much capacity it thinks it must reserve.
fn setup() -> (Contract, NearToken) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(account(REGISTRY))
        .predecessor_account_id(account(OWNER))
        .account_balance(NearToken::from_near(100))
        .block_height(HEIGHT)
        .build());
    let mut contract = Contract::new();
    let initial_usage = env::storage_usage();
    let longest_owner = "a".repeat(AccountId::MAX_LEN);
    let longest_proposal = "b".repeat(AccountId::MAX_LEN);
    let funded = NearToken::from_near(100);
    receipt(OWNER, NearToken::from_yoctonear(1), funded);
    contract.own_propose_owner(Some(account(&longest_owner)));
    receipt(&longest_owner, NearToken::from_yoctonear(1), funded);
    contract.own_accept_owner();
    contract.own_propose_owner(Some(account(&longest_proposal)));
    let maximum_usage = env::storage_usage();
    contract.own_propose_owner(Some(account(OWNER)));
    receipt(OWNER, NearToken::from_yoctonear(1), funded);
    contract.own_accept_owner();
    assert_eq!(env::storage_usage(), initial_usage);
    assert_eq!(contract.own_get_owner(), Some(account(OWNER)));
    assert_eq!(contract.own_get_proposed_owner(), None);
    let owner_capacity = storage_cost(maximum_usage.checked_sub(initial_usage).unwrap());
    assert!(owner_capacity > NearToken::ZERO);
    (contract, owner_capacity)
}

fn register(contract: &mut Contract, credit: NearToken) {
    let deposit = contract
        .storage_balance_bounds()
        .min
        .checked_add(credit)
        .unwrap();
    receipt(PAYER, deposit, NearToken::from_near(100));
    assert_eq!(contract.storage_deposit(None, None).available, credit);
}

/// A read-only boundary probe is safe even with native mocks: neither branch
/// mutates storage. Public operations below establish which liabilities exist.
fn assert_free_balance(contract: &Contract, free: NearToken) {
    contract.assert_storage_solvency(free);
    assert_panics(SOLVENCY_ERROR, || {
        contract.assert_storage_solvency(free.checked_add(NearToken::from_yoctonear(1)).unwrap());
    });
}

#[rstest]
fn deposit_cannot_use_new_credits_to_cover_unfunded_protocol_liabilities(
    #[values(false, true)] almost_funded_owner_capacity: bool,
) {
    let (mut contract, owner_capacity) = setup();
    let credit = NearToken::from_near(1);
    let deposit = contract
        .storage_balance_bounds()
        .min
        .checked_add(credit)
        .unwrap();
    // The incoming deposit pays for its registration and withdrawable credit,
    // not for an absent Owner-slot reserve or a one-yocto protocol deficit.
    let protocol_capacity = if almost_funded_owner_capacity {
        owner_capacity
            .checked_sub(NearToken::from_yoctonear(1))
            .unwrap()
    } else {
        NearToken::ZERO
    };
    let balance = physical_storage_cost()
        .checked_add(protocol_capacity)
        .unwrap()
        .checked_add(deposit)
        .unwrap();
    receipt(PAYER, deposit, balance);
    assert_panics(SOLVENCY_ERROR, || {
        contract.storage_deposit(None, None);
    });
    // A panicking native call does not roll storage back; discard this fixture.
}

#[rstest]
fn protocol_writes_cannot_spend_withdrawable_user_credits(#[values(false, true)] role_write: bool) {
    let (mut contract, owner_capacity) = setup();
    let credit = NearToken::from_near(1);
    register(&mut contract, credit);
    let balance = physical_storage_cost()
        .checked_add(owner_capacity)
        .unwrap()
        .checked_add(credit)
        .unwrap();
    receipt(OWNER, NearToken::from_yoctonear(1), balance);
    assert_eq!(
        contract
            .storage_balance_of(account(PAYER))
            .unwrap()
            .available,
        credit
    );
    assert_free_balance(&contract, NearToken::ZERO);
    assert_panics(SOLVENCY_ERROR, || {
        if role_write {
            contract.set_role(account("operator.near"), Role::Uploader, Some(true), None);
        } else {
            let _ = contract.add_version(
                "protocol-write".into(),
                VersionSource::Stored(vec![7].into()),
            );
        }
    });
    // In particular, do not assert rollback after either mutating failure.
}

#[test]
fn standard_owner_growth_cancellation_and_acceptance_keep_credits_withdrawable() {
    let (mut contract, owner_capacity) = setup();
    let credit = NearToken::from_near(1);
    register(&mut contract, credit);
    let minimum = contract.storage_balance_bounds().min;
    let mut balance = physical_storage_cost()
        .checked_add(owner_capacity)
        .unwrap()
        .checked_add(credit)
        .unwrap();
    let longest_owner = "a".repeat(AccountId::MAX_LEN);
    let longest_proposal = "b".repeat(AccountId::MAX_LEN);
    let installment = NearToken::from_millinear(250);
    let mut remaining = credit;

    for stage in 0..4 {
        let owner = if stage == 3 {
            longest_owner.as_str()
        } else {
            OWNER
        };
        receipt(owner, NearToken::from_yoctonear(1), balance);
        match stage {
            0 => contract.own_propose_owner(Some(account(&longest_owner))),
            1 => contract.own_propose_owner(None),
            2 => {
                contract.own_propose_owner(Some(account(&longest_owner)));
                receipt(&longest_owner, NearToken::from_yoctonear(1), balance);
                contract.own_accept_owner();
            }
            3 => contract.own_propose_owner(Some(account(&longest_proposal))),
            _ => unreachable!(),
        }
        assert_eq!(
            contract.own_get_owner(),
            Some(account(if stage >= 2 { &longest_owner } else { OWNER }))
        );
        let proposal = match stage {
            0 => Some(account(&longest_owner)),
            3 => Some(account(&longest_proposal)),
            _ => None,
        };
        assert_eq!(contract.own_get_proposed_owner(), proposal);
        assert_free_balance(&contract, NearToken::ZERO);

        receipt(PAYER, NearToken::from_yoctonear(1), balance);
        let withdrawn = contract.storage_withdraw(Some(installment));
        remaining = remaining.checked_sub(installment).unwrap();
        assert_eq!(withdrawn.available, remaining);
        assert_eq!(withdrawn.total, minimum.checked_add(remaining).unwrap());
        assert_eq!(transfers(), vec![(account(PAYER), installment)]);
        // Mocked receipts do not debit transfers. Explicitly carry their actual
        // amount into the next receipt's already-deposit-inclusive balance.
        balance = balance.checked_sub(installment).unwrap();
        receipt(PAYER, NearToken::from_yoctonear(1), balance);
        assert_free_balance(&contract, NearToken::ZERO);
    }
    assert_eq!(
        contract
            .storage_balance_of(account(PAYER))
            .unwrap()
            .available,
        NearToken::ZERO
    );
}

struct InFlight {
    contract: Contract,
    balance: NearToken,
    child: AccountId,
    deployment: Deployment,
    final_growth: NearToken,
    write_cost: NearToken,
}

fn in_flight(fund_intervening_write: bool) -> InFlight {
    let (mut contract, owner_capacity) = setup();
    let credit = NearToken::from_near(1);
    register(&mut contract, credit);
    receipt(
        OWNER,
        NearToken::from_yoctonear(1),
        NearToken::from_near(100),
    );
    let _ = contract.add_version("v".into(), VersionSource::Stored(vec![0].into()));
    let before_write = env::storage_usage();
    let _ = contract.add_version("calibrate".into(), VersionSource::Stored(vec![7].into()));
    let write_cost = storage_cost(env::storage_usage().checked_sub(before_write).unwrap());
    assert!(write_cost > NearToken::ZERO);
    let deployment = Deployment {
        version_key: "v".into(),
        code_hash: env::sha256_array([0]).into(),
        block_height: HEIGHT.into(),
    };
    // Derive the liability from the serialized public deployment record and
    // the actual reservation representation, not the aggregate reserve field.
    let final_growth = storage_cost(
        u64::try_from(
            borsh::object_length(&RegistryEntry::Deployed(deployment.clone())).unwrap()
                - borsh::object_length(&RegistryEntry::ReservedWithStorage(NearToken::ZERO))
                    .unwrap(),
        )
        .unwrap(),
    );
    assert!(final_growth > NearToken::ZERO);
    receipt(OWNER, NearToken::from_near(1), NearToken::from_near(100));
    contract
        .deploy("child".into(), "v".into(), vec![].into(), None)
        .detach();
    let child = account("child.registry.near");
    assert_eq!(
        contract.get_registry_entry(child.clone()),
        Some(RegistryEntryView::Reserved)
    );
    assert_eq!(contract.get_deployment(child.clone()), None);
    assert!(contract.list_deployments(None, None).is_empty());
    let mut balance = physical_storage_cost()
        .checked_add(owner_capacity)
        .unwrap()
        .checked_add(credit)
        .unwrap()
        .checked_add(final_growth)
        .unwrap();
    if fund_intervening_write {
        balance = balance.checked_add(write_cost).unwrap();
    }
    // Choose a tightly funded callback-era context, with the child transfer
    // already paid. No sandbox execution or administrative withdrawal is implied.
    receipt(OWNER, NearToken::from_yoctonear(1), balance);
    InFlight {
        contract,
        balance,
        child,
        deployment,
        final_growth,
        write_cost,
    }
}

#[test]
fn a_protocol_write_cannot_consume_an_in_flight_deployments_final_record_budget() {
    let mut fixture = in_flight(false);
    assert_free_balance(&fixture.contract, NearToken::ZERO);
    assert_panics(SOLVENCY_ERROR, || {
        let _ = fixture
            .contract
            .add_version("intervene".into(), VersionSource::Stored(vec![7].into()));
    });
    // The failed protocol write mutates mock storage; never finalize this fixture.
}

#[rstest]
fn finalization_releases_its_budget_after_an_intervening_protocol_write(
    #[values(false, true)] successful: bool,
) {
    let mut fixture = in_flight(true);
    assert_free_balance(&fixture.contract, fixture.write_cost);
    let before_write = env::storage_usage();
    let _ = fixture
        .contract
        .add_version("intervene".into(), VersionSource::Stored(vec![7].into()));
    assert_eq!(
        storage_cost(env::storage_usage() - before_write),
        fixture.write_cost
    );
    assert!(fixture.contract.get_version("intervene".into()).is_some());
    assert_free_balance(&fixture.contract, NearToken::ZERO);
    let before_finalize = env::storage_usage();
    let result = if successful {
        PromiseResult::Successful(vec![])
    } else {
        PromiseResult::Failed
    };
    testing_env!(
        context(REGISTRY, NearToken::ZERO, fixture.balance).build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        std::collections::HashMap::default(),
        vec![result],
    );
    let finalized = fixture.contract.deploy_01_finalize(
        Some(fixture.child.clone()),
        None,
        fixture.deployment.clone(),
        None,
        None,
    );
    let free = if successful {
        assert!(matches!(&finalized, PromiseOrValue::Value(child) if child == &fixture.child));
        assert_eq!(
            fixture.contract.get_registry_entry(fixture.child.clone()),
            Some(RegistryEntryView::Deployed(fixture.deployment.clone()))
        );
        assert_eq!(
            fixture.contract.get_deployment(fixture.child.clone()),
            Some(&fixture.deployment)
        );
        assert_eq!(
            fixture.contract.list_deployments(None, None),
            vec![&fixture.child]
        );
        assert_eq!(
            storage_cost(env::storage_usage() - before_finalize),
            fixture.final_growth
        );
        NearToken::ZERO
    } else {
        assert!(matches!(finalized, PromiseOrValue::Promise(_)));
        assert_eq!(
            fixture.contract.get_registry_entry(fixture.child.clone()),
            None
        );
        assert_eq!(fixture.contract.get_deployment(fixture.child.clone()), None);
        assert!(fixture.contract.list_deployments(None, None).is_empty());
        storage_cost(before_finalize - env::storage_usage())
            .checked_add(fixture.final_growth)
            .unwrap()
    };
    // Success spends the held budget on real storage; failure makes both that
    // budget and the removed reservation's storage available for protocol use.
    assert_free_balance(&fixture.contract, free);
    receipt(PAYER, NearToken::from_yoctonear(1), fixture.balance);
    let withdrawn = fixture.contract.storage_withdraw(None);
    assert_eq!(withdrawn.available, NearToken::ZERO);
    assert_eq!(
        withdrawn.total,
        fixture.contract.storage_balance_bounds().min
    );
    assert_eq!(transfers(), vec![(account(PAYER), NearToken::from_near(1))]);
    receipt(
        PAYER,
        NearToken::from_yoctonear(1),
        fixture
            .balance
            .checked_sub(NearToken::from_near(1))
            .unwrap(),
    );
    assert_free_balance(&fixture.contract, free);
}

#[test]
fn failed_finalization_cleans_up_even_when_protocol_backing_is_short() {
    let mut fixture = in_flight(false);
    let payer_balance = fixture.contract.storage_balance_of(account(PAYER)).unwrap();
    let available_credit = fixture.contract.available_storage_balance;
    let short_balance = fixture
        .balance
        .checked_sub(payer_balance.available)
        .unwrap();
    testing_env!(
        context(REGISTRY, NearToken::ZERO, short_balance).build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        std::collections::HashMap::default(),
        vec![PromiseResult::Failed],
    );

    let _ = fixture.contract.deploy_01_finalize(
        Some(fixture.child.clone()),
        None,
        fixture.deployment,
        None,
        None,
    );
    assert_eq!(fixture.contract.get_registry_entry(fixture.child), None);
    assert_eq!(fixture.contract.pending_deployment_storage, NearToken::ZERO);
    assert_eq!(
        fixture.contract.storage_balance_of(account(PAYER)),
        Some(payer_balance)
    );
    assert_eq!(fixture.contract.available_storage_balance, available_credit);
    // Cleanup repairs the reservation; it does not authorize new spending.
    receipt(OWNER, NearToken::from_yoctonear(1), short_balance);
    assert_panics(SOLVENCY_ERROR, || {
        let _ = fixture
            .contract
            .add_version("still-short".into(), VersionSource::Stored(vec![7].into()));
    });
}

#[test]
fn failed_global_finalization_frees_key_when_protocol_backing_is_short() {
    let (mut contract, _owner_capacity) = setup();
    register(&mut contract, NearToken::from_near(1));
    receipt(
        OWNER,
        NearToken::from_yoctonear(1),
        NearToken::from_near(100),
    );
    let _ = contract.add_version(
        "retry-global".into(),
        VersionSource::ExistingGlobal([7; 32].into()),
    );
    assert!(contract.get_version("retry-global".into()).is_some());

    // The callback omits the payer's available credit from its balance. Cleanup
    // only removes the provisional GlobalHash; it must still release the key.
    let short_balance = physical_storage_cost();
    testing_env!(
        context(REGISTRY, NearToken::ZERO, short_balance).build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        std::collections::HashMap::default(),
        vec![PromiseResult::Failed],
    );
    let finalized = contract.add_version_01_finalize("retry-global".into());
    assert!(matches!(finalized, PromiseOrValue::Promise(_)));
    assert_eq!(contract.get_version("retry-global".into()), None);
    assert!(contract.list_versions(None, None).is_empty());
}
