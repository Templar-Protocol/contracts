use near_sdk::{json_types::Base64VecU8, test_utils::VMContextBuilder, testing_env, NearToken};
use near_sdk_contract_tools::{
    owner::{OwnerExternal, OwnerInternal},
    rbac::Rbac,
};
use templar_common::{
    oracle::redstone::{config, Config, RedStoneAdapter, Role},
    upgrade::UpgradeSource,
};
use templar_redstone_adapter_contract::Contract;

#[allow(
    clippy::unwrap_used,
    reason = "invalid test account literals must fail immediately"
)]
fn context(caller: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id("adapter.near".parse().unwrap())
        .predecessor_account_id(caller.parse().unwrap())
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build());
}

fn roles(contract: &Contract) -> [Vec<near_sdk::AccountId>; 2] {
    [Role::ModifyRoles, Role::TrustedUpdater].map(|role| contract.list_role(role, None, None))
}

#[allow(
    clippy::unwrap_used,
    reason = "asserting the rejected call's panic and diagnostic"
)]
fn rejected(f: impl FnOnce(), message: &str) {
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
    let text = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(text.contains(message), "{text}");
}

#[test]
fn bootstrap_preserves_legacy_root_configuration_and_rbac() {
    context("adapter.near", 0);
    // Historical decoding is intentionally not subject to the new constructor's validation.
    let config = Config {
        signer_count_threshold: 0,
        ..config::prod()
    };
    let mut contract = Contract {
        adapter: RedStoneAdapter::new(b"a", config.clone()),
    };
    for (account, role) in [
        ("old-admin.near", Role::ModifyRoles),
        ("extra-admin.near", Role::ModifyRoles),
        ("writer.near", Role::TrustedUpdater),
    ] {
        Rbac::add_role(&mut contract, &account.parse().unwrap(), &role);
    }
    let before = near_sdk::borsh::to_vec(&contract).unwrap();
    let members = roles(&contract);
    rejected(
        || contract.migrate(None),
        "migrate: owner_id is required for uninitialized ownership",
    );
    assert!(!Contract::slot_is_initialized().exists());
    assert_eq!(near_sdk::borsh::to_vec(&contract).unwrap(), before);
    assert_eq!(roles(&contract), members);

    contract.migrate(Some("dao.near".parse().unwrap()));
    assert_eq!(contract.own_get_owner(), Some("dao.near".parse().unwrap()));
    assert!(Contract::slot_is_initialized().exists());
    assert_eq!(contract.get_config(), &config);
    assert_eq!(near_sdk::borsh::to_vec(&contract).unwrap(), before);
    assert_eq!(roles(&contract), members);
    assert!(!contract.has_role("dao.near".parse().unwrap(), Role::ModifyRoles));

    rejected(
        || contract.migrate(Some("other.near".parse().unwrap())),
        "migrate: owner is already initialized",
    );
    contract.migrate(None);
    assert_eq!(contract.own_get_owner(), Some("dao.near".parse().unwrap()));
    assert_eq!(roles(&contract), members);
}

#[test]
fn migration_preserves_pending_transfer_and_cannot_rebootstrap_renunciation() {
    context("owner.near", 1);
    let mut contract = Contract::new(config::prod(), "owner.near".parse().unwrap());
    let members = roles(&contract);
    contract.own_propose_owner(Some("next.near".parse().unwrap()));
    context("adapter.near", 0);
    contract.migrate(None);
    assert_eq!(
        contract.own_get_owner(),
        Some("owner.near".parse().unwrap())
    );
    assert_eq!(
        contract.own_get_proposed_owner(),
        Some("next.near".parse().unwrap())
    );
    assert_eq!(roles(&contract), members);

    context("next.near", 1);
    contract.own_accept_owner();
    assert_eq!(contract.own_get_owner(), Some("next.near".parse().unwrap()));
    assert_eq!(contract.own_get_proposed_owner(), None);
    assert_eq!(
        roles(&contract),
        members,
        "ownership must not transfer RBAC"
    );
    contract.own_propose_owner(Some("pending.near".parse().unwrap()));
    contract.own_renounce_owner();
    assert_eq!(contract.own_get_owner(), None);
    assert_eq!(contract.own_get_proposed_owner(), None);
    context("adapter.near", 0);
    contract.migrate(None);
    rejected(
        || contract.migrate(Some("owner.near".parse().unwrap())),
        "migrate: owner is already initialized",
    );
    assert!(Contract::slot_is_initialized().exists());
    assert_eq!(contract.own_get_owner(), None);
    assert_eq!(roles(&contract), members);
}

#[rstest::rstest]
#[case::stranger("stranger.near", 1, "Owner only")]
#[case::role_administrator("roles.near", 1, "Owner only")]
#[case::updater("writer.near", 1, "Owner only")]
#[case::zero_deposit("owner.near", 0, "exactly 1 yoctoNEAR")]
#[case::excess_deposit("owner.near", 2, "exactly 1 yoctoNEAR")]
#[case::empty_code("owner.near", 1, "admin_upgrade: empty code blob")]
fn upgrade_requires_owner_and_exact_deposit(
    #[case] caller: &str,
    #[case] deposit: u128,
    #[case] message: &str,
) {
    context("owner.near", 0);
    let mut contract = Contract::new(config::prod(), "owner.near".parse().unwrap());
    Rbac::add_role(
        &mut contract,
        &"roles.near".parse().unwrap(),
        &Role::ModifyRoles,
    );
    Rbac::add_role(
        &mut contract,
        &"writer.near".parse().unwrap(),
        &Role::TrustedUpdater,
    );
    let before = near_sdk::borsh::to_vec(&contract).unwrap();
    let members = roles(&contract);
    context(caller, deposit);
    rejected(
        || {
            contract
                .admin_upgrade(
                    UpgradeSource::Code(Base64VecU8(vec![])),
                    Base64VecU8(b"{}".to_vec()),
                )
                .detach();
        },
        message,
    );
    assert_eq!(near_sdk::borsh::to_vec(&contract).unwrap(), before);
    assert_eq!(roles(&contract), members);
    assert_eq!(
        contract.own_get_owner(),
        Some("owner.near".parse().unwrap())
    );
}

#[rstest::rstest]
#[case::zero_quorum(Config { signer_count_threshold: 0, ..config::prod() })]
#[case::empty_signers(Config { signers: vec![], ..config::prod() })]
fn constructor_rejects_invalid_configuration_before_ownership(#[case] config: Config) {
    context("registry.near", 0);
    assert!(
        std::panic::catch_unwind(|| Contract::new(config, "operator.near".parse().unwrap()))
            .is_err()
    );
    assert!(!Contract::slot_is_initialized().exists());
}
