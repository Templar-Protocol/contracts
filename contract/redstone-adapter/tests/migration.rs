use near_sdk::{
    env, json_types::Base64VecU8, test_utils::VMContextBuilder, testing_env, NearToken,
};
use near_sdk_contract_tools::rbac::Rbac;
use templar_common::{
    oracle::redstone::{config, Config, RedStoneAdapter, RedStoneContractInterface, Role},
    upgrade::UpgradeSource,
    versioned_state::{
        parse_one_or_many, read_state_version, run_migration_chain, write_state_version,
        MigrateExternalInterface, StateTransformer, StateVersion,
    },
};
use templar_redstone_adapter_contract::{
    state::{
        legacy::v0,
        migration::{Migration, V0ToV1},
        State,
    },
    Contract,
};

#[allow(
    clippy::unwrap_used,
    reason = "invalid test account literals must fail immediately"
)]
fn context(caller: &str, deposit: u128) {
    let context = VMContextBuilder::new()
        .current_account_id("adapter.near".parse().unwrap())
        .predecessor_account_id(caller.parse().unwrap())
        .block_timestamp(1_770_985_144_000_000_000)
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build();
    testing_env!(context);
}

fn roles(contract: &Contract) -> [Vec<near_sdk::AccountId>; 2] {
    [Role::Admin, Role::TrustedUpdater].map(|role| contract.list_role(role, None, None))
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
fn typed_migration_moves_legacy_root_without_validating_historical_policy() {
    context("adapter.near", 0);
    let config = Config {
        signer_count_threshold: 0,
        ..config::prod()
    };
    let legacy = v0::State {
        adapter: RedStoneAdapter::new(b"a", config.clone()),
    };
    let before = near_sdk::borsh::to_vec(&legacy).unwrap();
    env::state_write(&legacy);
    let migrated = V0ToV1.run().unwrap();
    assert_eq!(migrated.adapter.config, config);
    assert_eq!(near_sdk::borsh::to_vec(&migrated).unwrap(), before);
    assert_eq!(env::storage_read(b"STATE").unwrap(), before);
    assert_eq!(read_state_version().unwrap(), 1);
}

#[test]
fn typed_chain_preserves_legacy_feeds_roles_and_root_bytes() {
    context("adapter.near", 0);
    let legacy = v0::State {
        adapter: RedStoneAdapter::new(b"a", config::prod()),
    };
    // The version wrapper is transparent; this facade seeds the same legacy storage.
    let mut contract: Contract =
        near_sdk::borsh::from_slice(&near_sdk::borsh::to_vec(&legacy).unwrap()).unwrap();
    for (account, role) in [
        ("old-admin.near", Role::Admin),
        ("extra-admin.near", Role::Admin),
        ("writer.near", Role::TrustedUpdater),
    ] {
        Rbac::add_role(&mut contract, &account.parse().unwrap(), &role);
    }
    let fixture = include_str!("fixtures/eth_btc.hex").trim();
    let payload = (0..fixture.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&fixture[offset..offset + 2], 16).unwrap())
        .collect::<Vec<_>>();
    contract.write_prices(vec!["ETH".into(), "BTC".into()], payload.into());
    env::state_write(&contract);
    let before = env::storage_read(b"STATE").unwrap();
    let members = roles(&contract);
    let feeds = contract.read_price_data(vec!["ETH".into(), "BTC".into()]);
    assert_eq!(feeds.len(), 2);
    assert_eq!(Contract::get_stored_state_version(), 0);
    assert_eq!(Contract::get_target_state_version(), 1);
    assert!(Contract::needs_migration());
    drop(contract);

    assert!(parse_one_or_many::<Migration>(b"{}").is_err());
    for input in [
        &b"[]"[..],
        &br#"[{"from_version":"v0"},{"from_version":"v0"}]"#[..],
    ] {
        let migrations = parse_one_or_many::<Migration>(input).unwrap();
        assert!(run_migration_chain(migrations, State::VERSION).is_err());
        assert_eq!(env::storage_read(b"STATE").unwrap(), before);
        assert_eq!(read_state_version().unwrap(), 0);
    }

    let migrations = parse_one_or_many::<Migration>(br#"{"from_version":"v0"}"#).unwrap();
    run_migration_chain(migrations, State::VERSION).unwrap();
    assert_eq!(env::storage_read(b"STATE").unwrap(), before);
    assert_eq!(Contract::get_stored_state_version(), 1);
    assert!(!Contract::needs_migration());
    let mut contract: Contract = env::state_read().unwrap();
    assert_eq!(roles(&contract), members);
    assert_eq!(
        contract.read_price_data(vec!["ETH".into(), "BTC".into()]),
        feeds
    );
    context("old-admin.near", 1);
    contract.admin_set_config(config::prod());
    assert_eq!(roles(&contract), members);
}

#[test]
fn future_state_versions_are_not_treated_as_current() {
    context("admin.near", 0);
    let contract = Contract::new(config::prod(), "admin.near".parse().unwrap());
    env::state_write(&contract);
    let before = env::storage_read(b"STATE").unwrap();
    assert_eq!(Contract::get_stored_state_version(), 1);
    assert_eq!(Contract::get_target_state_version(), 1);
    assert!(!Contract::needs_migration());
    write_state_version(2);
    assert!(State::needs_migration().is_err());
    assert_eq!(env::storage_read(b"STATE").unwrap(), before);
    assert_eq!(read_state_version().unwrap(), 2);
}

#[rstest::rstest]
#[case::stranger("stranger.near", 1, "Unauthorized role")]
#[case::updater("writer.near", 1, "Unauthorized role")]
#[case::zero_deposit("admin.near", 0, "exactly 1 yoctoNEAR")]
#[case::excess_deposit("admin.near", 2, "exactly 1 yoctoNEAR")]
#[case::empty_code("admin.near", 1, "admin_upgrade: empty code blob")]
fn upgrade_requires_admin_and_exact_deposit(
    #[case] caller: &str,
    #[case] deposit: u128,
    #[case] message: &str,
) {
    context("admin.near", 0);
    let mut contract = Contract::new(config::prod(), "admin.near".parse().unwrap());
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
                    Base64VecU8(vec![]),
                )
                .detach();
        },
        message,
    );
    assert_eq!(near_sdk::borsh::to_vec(&contract).unwrap(), before);
    assert_eq!(roles(&contract), members);
    assert_eq!(read_state_version().unwrap(), 1);
}

#[rstest::rstest]
#[case::zero_quorum(Config { signer_count_threshold: 0, ..config::prod() })]
#[case::empty_signers(Config { signers: vec![], ..config::prod() })]
fn constructor_rejects_invalid_configuration_before_initializing_state(#[case] config: Config) {
    context("registry.near", 0);
    assert!(
        std::panic::catch_unwind(|| Contract::new(config, "operator.near".parse().unwrap()))
            .is_err()
    );
    assert!(!env::storage_has_key(b"__v"));
    for role in [Role::Admin, Role::TrustedUpdater] {
        assert!(!<Contract as Rbac>::has_role(
            &"operator.near".parse().unwrap(),
            &role,
        ));
    }
}
